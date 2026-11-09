//! The daemon's IPC server: newline JSON over a local socket (Unix socket or named pipe), one task
//! per connection, each request dispatched onto [`Engine`]. Port of Go `internal/daemon/server.go`
//! plus the new `reload`, `subscribe`, `version` and `doctor` commands (plan §I).
//!
//! Start-up order (Task 36): [`lock::acquire`](super::lock::acquire), then [`bind`] (which needs
//! the [`LockGuard`], so a stale socket is only unlinked under the lock), then [`Server::serve`].
//!
//! Wire rules kept from Go:
//! - One response line per request line, in order, on the same connection.
//! - A line that is not JSON gets `{"ok":false,"error":"invalid JSON: ..."}` and the connection
//!   stays open. Unknown keys are ignored; a missing `cmd` is `unknown command: ""`.
//! - `pause`, `resume` and `favorite` answer `{"ok":true}` (no `data`).
//! - Every line goes through `codec::write_line_json` (Go's `<`/`>`/`&` escaping).

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use interprocess::local_socket::tokio::Listener;
use interprocess::local_socket::traits::tokio::Listener as _;
use kakemono_config::{Config, ConfigError};
use kakemono_engine::{Engine, EngineEvent};
use kakemono_platform::api::BackendInfo;
use kakemono_platform::linux::detect::Env;
use kakemono_protocol::client;
use kakemono_protocol::codec::{self, CodecError};
use kakemono_protocol::endpoint::Endpoint;
use kakemono_protocol::types::{Command, Event, Request, Response};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::lock::LockGuard;

/// How long [`Server::serve`] waits for open connections to finish after shutdown before
/// aborting them.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Pause after a failed `accept` (e.g. out of file descriptors) so the loop doesn't spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// Re-reads the config from disk for `reload`. The daemon (Task 36) builds it from the same
/// `--config` / `Paths::locate_config` choice it started with.
pub type ConfigLoader = Arc<dyn Fn() -> Result<Config, ConfigError> + Send + Sync>;

/// Captures the daemon's own detection environment for `doctor` (Task 37; Linux). The daemon
/// installs `detect::env::capture_with_dbus`; tests leave it out so payloads are the same on
/// every OS.
pub type EnvProbe = Arc<dyn Fn() -> BoxFuture<'static, Env> + Send + Sync>;

/// Payload of `version`: `{"version":"<crate version>"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionData {
    pub version: String,
}

impl VersionData {
    /// This build's version.
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// Payload of `doctor`: `{"version","pid","backend":{"name","detail"},"theme"}`, plus `env`
/// when an [`EnvProbe`] is installed and `apply` when the request asked for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorData {
    pub version: String,
    pub pid: u32,
    pub backend: DoctorBackend,
    pub theme: String,
    /// What detection sees from inside the daemon (its env, not the shell's). Linux only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Env>,
    /// The outcome of re-setting the current wallpaper (`doctor` with `apply`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply: Option<ApplyOutcome>,
}

/// [`BackendInfo`], readable back by the CLI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorBackend {
    pub name: String,
    pub detail: String,
}

impl From<BackendInfo> for DoctorBackend {
    fn from(info: BackendInfo) -> Self {
        Self {
            name: info.name,
            detail: info.detail,
        }
    }
}

/// `{"ok":true,"path":..}` or `{"ok":false,"error":..}`. A failed apply is still an `ok:true`
/// response: the rest of the report is what the user needs to see why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyOutcome {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The `error` of [`ApplyOutcome`] when there is no current wallpaper to re-set.
pub const NOTHING_TO_APPLY: &str = "no current wallpaper to re-set; run `kakemono next` first";

/// Why [`bind`] failed.
#[derive(Debug, thiserror::Error)]
pub enum BindError {
    /// Something answers on the endpoint although this process holds the lock: a daemon using a
    /// different runtime dir, or the Go `wallpaperd` daemon on a shared `socket_path`.
    #[error(
        "another process is already listening on {0}; stop it, or choose another socket with `socket_path` or KAKEMONO_SOCKET"
    )]
    InUse(PathBuf),
    /// The endpoint path exists but is not a socket. Never deleted.
    #[error("{0} exists and is not a socket; refusing to remove it")]
    NotASocket(PathBuf),
    #[error("server: remove stale socket {}: {source}", path.display())]
    RemoveStale { path: PathBuf, source: io::Error },
    #[error("server: listen on {}: {source}", path.display())]
    Listen { path: PathBuf, source: io::Error },
}

/// Binds `endpoint`. Holding `_lock` proves no other kakemono daemon runs, so a leftover socket
/// file from a crashed daemon is removed first (Go removed it unconditionally, which let a second
/// foreground daemon steal a live socket). A socket that still accepts connections is left alone
/// ([`BindError::InUse`]). `Endpoint::bind` creates the parent dir 0700 and the socket 0600; the
/// listener unlinks the socket file when dropped.
pub async fn bind(endpoint: &Endpoint, _lock: &LockGuard) -> Result<Listener, BindError> {
    remove_stale(endpoint).await?;
    endpoint.bind().map_err(|source| BindError::Listen {
        path: endpoint.path().to_owned(),
        source,
    })
}

#[cfg(unix)]
async fn remove_stale(endpoint: &Endpoint) -> Result<(), BindError> {
    use std::os::unix::fs::FileTypeExt;
    let path = endpoint.path();
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(BindError::RemoveStale {
                path: path.to_owned(),
                source,
            });
        }
    };
    if !meta.file_type().is_socket() {
        return Err(BindError::NotASocket(path.to_owned()));
    }
    if client::is_running(endpoint).await {
        return Err(BindError::InUse(path.to_owned()));
    }
    tracing::info!(socket = %path.display(), "removing stale socket");
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(BindError::RemoveStale {
            path: path.to_owned(),
            source,
        }),
    }
}

#[cfg(not(unix))]
async fn remove_stale(endpoint: &Endpoint) -> Result<(), BindError> {
    // Named pipes leave nothing behind; only refuse a live one.
    if client::is_running(endpoint).await {
        return Err(BindError::InUse(endpoint.path().to_owned()));
    }
    Ok(())
}

/// Dispatches requests onto one [`Engine`]. Cheap to clone (one clone per connection).
#[derive(Clone)]
pub struct Server {
    engine: Engine,
    config_loader: ConfigLoader,
    env_probe: Option<EnvProbe>,
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("engine", &self.engine)
            .field("env_probe", &self.env_probe.is_some())
            .finish_non_exhaustive()
    }
}

impl Server {
    /// A server for `engine`. `reload` calls `config_loader` to re-read the config from disk.
    pub fn new(
        engine: Engine,
        config_loader: impl Fn() -> Result<Config, ConfigError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            engine,
            config_loader: Arc::new(config_loader),
            env_probe: None,
        }
    }

    /// Adds the daemon's environment snapshot ([`DoctorData::env`]) to `doctor`.
    pub fn with_env_probe(
        mut self,
        probe: impl Fn() -> BoxFuture<'static, Env> + Send + Sync + 'static,
    ) -> Self {
        self.env_probe = Some(Arc::new(probe));
        self
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Accept loop: one task per connection, until `shutdown` is cancelled. Then the listener is
    /// dropped (which unlinks the Unix socket), open connections are told to stop (subscribers
    /// end at once, a request in flight finishes) and are aborted after [`SHUTDOWN_GRACE`].
    pub async fn serve(self, listener: Listener, shutdown: CancellationToken) {
        let mut conns = JoinSet::new();
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                Some(joined) = conns.join_next(), if !conns.is_empty() => {
                    if let Err(e) = joined
                        && e.is_panic()
                    {
                        tracing::error!("server: connection task panicked: {e}");
                    }
                }
                accepted = listener.accept() => match accepted {
                    Ok(stream) => {
                        let server = self.clone();
                        let stop = shutdown.child_token();
                        conns.spawn(async move { server.serve_connection(stream, stop).await });
                    }
                    Err(e) => {
                        tracing::warn!("server: accept error: {e}");
                        tokio::time::sleep(ACCEPT_BACKOFF).await;
                    }
                },
            }
        }
        drop(listener);
        let drain = async { while conns.join_next().await.is_some() {} };
        if tokio::time::timeout(SHUTDOWN_GRACE, drain).await.is_err() {
            tracing::warn!("server: aborting connections still open after shutdown");
            conns.abort_all();
            while conns.join_next().await.is_some() {}
        }
    }

    /// Serves one connection until EOF, a transport error, or `shutdown`. Generic so tests can
    /// drive it over `tokio::io::duplex`.
    pub async fn serve_connection<S>(&self, stream: S, shutdown: CancellationToken)
    where
        S: AsyncRead + AsyncWrite + Send,
    {
        let (read, mut write) = tokio::io::split(stream);
        let mut read = BufReader::new(read);
        loop {
            let line = tokio::select! {
                () = shutdown.cancelled() => break,
                line = codec::read_line_json::<Command, _>(&mut read) => line,
            };
            let response = match line {
                Ok(Some(command)) => match Request::try_from(command) {
                    Ok(Request::Subscribe) => {
                        self.stream_events(&mut read, &mut write, &shutdown).await;
                        break;
                    }
                    Ok(request) => self.dispatch(request).await,
                    Err(e) => Response::err(e),
                },
                Ok(None) => break,
                Err(CodecError::Json(e)) => Response::err(format!("invalid JSON: {e}")),
                Err(e) => {
                    tracing::debug!("server: connection read error: {e}");
                    break;
                }
            };
            if let Err(e) = codec::write_line_json(&mut write, &response).await {
                tracing::debug!("server: write response: {e}");
                break;
            }
        }
        let _ = write.shutdown().await;
    }

    /// Runs one request and builds its response. `subscribe` is only meaningful on a connection
    /// ([`Server::serve_connection`]); here it is an error.
    pub async fn dispatch(&self, request: Request) -> Response {
        tracing::debug!(cmd = request.name(), "server: request");
        match request {
            Request::Next => data(self.engine.next().await),
            Request::Prev => data(self.engine.prev().await),
            Request::Set { path } => data(self.engine.set_file(Path::new(&path)).await),
            Request::Favorite => match self.engine.favorite().await {
                Ok(()) => Response::ok(),
                Err(e) => Response::err(e),
            },
            Request::Trash { expected } => {
                data(self.engine.trash_expected(expected.as_deref()).await)
            }
            Request::Pause => {
                self.engine.pause();
                Response::ok()
            }
            Request::Resume => {
                self.engine.resume();
                Response::ok()
            }
            Request::Status => data(Ok::<_, String>(self.engine.status())),
            Request::Reload => self.reload().await,
            Request::Version => data(Ok::<_, String>(VersionData::current())),
            Request::Doctor { apply } => self.doctor(apply).await,
            Request::Subscribe => Response::err("subscribe: only valid on a connection"),
        }
    }

    async fn reload(&self) -> Response {
        let cfg = match (self.config_loader)() {
            Ok(cfg) => cfg,
            Err(e) => return Response::err(format!("reload: {e}")),
        };
        let old = self.engine.config();
        if cfg.daemon.socket_path != old.daemon.socket_path {
            tracing::warn!("reload: socket_path changed; restart the daemon to use it");
        }
        if cfg.desktop != old.desktop {
            tracing::warn!("reload: [desktop] changed; restart the daemon to use the new backend");
        }
        match self.engine.reload(cfg).await {
            Ok(()) => {
                tracing::info!("config reloaded");
                Response::ok()
            }
            Err(e) => Response::err(format!("reload: {e}")),
        }
    }

    /// `doctor`: with `apply`, re-set the current wallpaper first (through the engine's op lock
    /// and the daemon's own setter), so `backend` reflects any re-detection it caused.
    async fn doctor(&self, apply: bool) -> Response {
        let apply = if apply {
            Some(match self.engine.reapply().await {
                Ok(Some(info)) => ApplyOutcome {
                    ok: true,
                    path: Some(info.path),
                    error: None,
                },
                Ok(None) => ApplyOutcome {
                    ok: false,
                    path: None,
                    error: Some(NOTHING_TO_APPLY.to_owned()),
                },
                Err(e) => ApplyOutcome {
                    ok: false,
                    path: None,
                    error: Some(e.to_string()),
                },
            })
        } else {
            None
        };
        let env = match &self.env_probe {
            Some(probe) => Some(probe().await),
            None => None,
        };
        data(Ok::<_, String>(DoctorData {
            version: VersionData::current().version,
            pid: std::process::id(),
            backend: self.engine.platform().setter.info().into(),
            theme: self.engine.theme().as_str().to_owned(),
            env,
            apply,
        }))
    }

    /// `subscribe`: one `{"ok":true}` line, then one `Event` line per engine event until the
    /// client disconnects, the engine goes away, or shutdown. Requests sent after `subscribe` are
    /// read and discarded (only to notice EOF). A lagging subscriber loses the oldest events.
    async fn stream_events<R, W>(&self, read: &mut R, write: &mut W, shutdown: &CancellationToken)
    where
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        // Subscribe before the handshake so no event after `{"ok":true}` is missed.
        let mut events = self.engine.subscribe();
        if codec::write_line_json(write, &Response::ok())
            .await
            .is_err()
        {
            return;
        }
        let mut discard = Vec::new();
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                line = codec::read_line(read, &mut discard) => match line {
                    Ok(true) => discard.clear(),
                    Ok(false) | Err(_) => return,
                },
                event = events.recv() => match event {
                    Ok(event) => {
                        if write_event(write, event).await.is_err() {
                            return;
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        tracing::warn!("server: subscriber lagged; dropped {n} events");
                    }
                    Err(RecvError::Closed) => return,
                },
            }
        }
    }
}

async fn write_event<W: AsyncWrite + Unpin + Send>(
    write: &mut W,
    event: EngineEvent,
) -> Result<(), CodecError> {
    codec::write_line_json(write, &Event::from(event)).await
}

/// `{"ok":true,"data":..}` or `{"ok":false,"error":..}` (Go's `internal: marshal response` text if
/// encoding fails).
fn data<T: Serialize, E: fmt::Display>(result: Result<T, E>) -> Response {
    match result {
        Ok(value) => Response::ok_data(&value)
            .unwrap_or_else(|e| Response::err(format!("internal: marshal response: {e}"))),
        Err(e) => Response::err(e),
    }
}

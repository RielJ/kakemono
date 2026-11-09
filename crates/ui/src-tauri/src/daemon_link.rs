//! The UI's one shared connection to the daemon.
//!
//! [`DaemonLink`] owns a lazily opened [`Client`] behind a mutex, so the tray, the Tauri
//! commands and later the windows all reuse one connection instead of dialling per click.
//! A daemon restart leaves the cached connection dead; the next request notices, reconnects
//! once and resends. A cold connect that fails is reported as is (no retry loop here).

use kakemono_protocol::client::{Client, ClientError};
use kakemono_protocol::endpoint::{self, Endpoint, EndpointError};
use kakemono_protocol::types::{Command, Response, StatusData};
use tokio::sync::Mutex;

/// Why a daemon call failed, as shown to the user.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// Transport failure: the daemon is not running, timed out or hung up.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// The daemon answered `{"ok":false,"error":...}`.
    #[error("{0}")]
    Daemon(String),
    /// The daemon answered `ok` but the payload did not decode.
    #[error("decoding {what} response: {source}")]
    Decode {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

impl LinkError {
    /// True when nothing listens on the endpoint.
    pub fn is_not_running(&self) -> bool {
        matches!(self, Self::Client(ClientError::DaemonNotRunning { .. }))
    }
}

/// Shared handle to the daemon. Cheap to share through `tauri::State` or an `Arc`.
#[derive(Debug)]
pub struct DaemonLink {
    endpoint: Endpoint,
    client: Mutex<Option<Client>>,
}

impl DaemonLink {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            client: Mutex::new(None),
        }
    }

    /// Resolves the endpoint as the daemon and the CLI do: the config's `socket_path` through
    /// [`kakemono_config::Config::effective_socket_path`], then `$KAKEMONO_SOCKET`, then the
    /// platform default. A legacy `wallpaperd` config's Go default socket is ignored, so the UI
    /// never talks to a still-running Go daemon. A config that fails to load only loses its
    /// `socket_path`; the error is logged.
    pub fn from_environment() -> Result<Self, EndpointError> {
        let socket_path = match kakemono_config::paths::Paths::detect() {
            Ok(paths) => socket_override(&paths),
            Err(err) => {
                tracing::warn!(error = %err, "cannot resolve config paths; using the default socket");
                None
            }
        };
        endpoint::default_endpoint(socket_path.as_deref()).map(Self::new)
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Sends one command and returns the raw response, including `ok:false` ones.
    ///
    /// Requests are serialised on the one connection. When a cached connection turns out
    /// to be dead (the write fails, or the daemon closes it before answering), the link
    /// reconnects and resends once. A fresh connection is never retried.
    pub async fn request(&self, command: impl Into<Command>) -> Result<Response, ClientError> {
        let command = command.into();
        let mut slot = self.client.lock().await;

        if let Some(client) = slot.as_mut() {
            match client.request(command.clone()).await {
                Ok(response) => return Ok(response),
                Err(err) if is_stale_connection(&err) => {
                    tracing::debug!(error = %err, "cached daemon connection is dead; reconnecting");
                    *slot = None;
                }
                Err(err) => {
                    *slot = None;
                    return Err(err);
                }
            }
        }

        let mut client = Client::connect(&self.endpoint).await?;
        let result = client.request(command).await;
        if result.is_ok() {
            *slot = Some(client);
        }
        result
    }

    /// Sends one command and maps `ok:false` to [`LinkError::Daemon`].
    pub async fn call(&self, command: impl Into<Command>) -> Result<Response, LinkError> {
        let response = self.request(command).await?;
        if response.ok {
            Ok(response)
        } else {
            Err(LinkError::Daemon(response.error))
        }
    }

    /// IPC `status`, decoded.
    pub async fn status(&self) -> Result<StatusData, LinkError> {
        let response = self.call(kakemono_protocol::types::Request::Status).await?;
        response
            .data_as::<StatusData>()
            .map(Option::unwrap_or_default)
            .map_err(|source| LinkError::Decode {
                what: "status",
                source,
            })
    }
}

/// A send failure or an EOF before the answer means the peer went away, so a resend on a
/// new connection is safe. Timeouts and malformed replies are not retried: the daemon may
/// have acted on the request.
fn is_stale_connection(err: &ClientError) -> bool {
    matches!(err, ClientError::Send(_) | ClientError::Closed)
}

/// The endpoint override from the config the daemon would read: `Some` only for a
/// [`SocketPath::Configured`](kakemono_config::paths::SocketPath::Configured) path. Logs the
/// legacy notice once (this runs once per process).
fn socket_override(paths: &kakemono_config::paths::Paths) -> Option<std::path::PathBuf> {
    match paths.load_config(None) {
        Ok((config, source)) => {
            tracing::debug!(config = %source.path().display(), "loaded config");
            let socket = config.effective_socket_path(&source, &paths.home);
            if let Some(line) = socket.notice() {
                tracing::info!("{line}");
            }
            socket.override_path().map(ToOwned::to_owned)
        }
        Err(err) => {
            tracing::warn!(error = %err, "cannot load config; using the default socket");
            None
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use interprocess::local_socket::tokio::prelude::*;
    use kakemono_protocol::types::Request;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn temp_endpoint() -> (tempfile::TempDir, Endpoint) {
        let dir = tempfile::tempdir().expect("tempdir");
        let endpoint = Endpoint::new(dir.path().join("run/k.sock")).expect("endpoint");
        (dir, endpoint)
    }

    /// Serves one connection: answers each request line with the next canned reply, then
    /// hangs up. Returns how many request lines it read.
    fn serve(
        listener: LocalSocketListener,
        replies: Vec<&'static str>,
    ) -> tokio::task::JoinHandle<usize> {
        tokio::spawn(async move {
            let conn = listener.accept().await.expect("accept");
            let mut reader = BufReader::new(conn);
            let mut seen = 0;
            for reply in replies {
                let mut line = String::new();
                if reader.read_line(&mut line).await.expect("read") == 0 {
                    break;
                }
                seen += 1;
                reader
                    .get_mut()
                    .write_all(reply.as_bytes())
                    .await
                    .expect("write");
            }
            seen
        })
    }

    #[tokio::test]
    async fn reuses_one_connection_for_many_requests() {
        let (_dir, endpoint) = temp_endpoint();
        let listener = endpoint.bind().expect("bind");
        let server = serve(listener, vec!["{\"ok\":true}\n", "{\"ok\":true}\n"]);
        let link = DaemonLink::new(endpoint);

        link.call(Request::Next).await.expect("first");
        link.call(Request::Prev)
            .await
            .expect("second, same connection");
        assert_eq!(server.await.expect("server"), 2);
    }

    #[tokio::test]
    async fn reconnects_once_after_the_daemon_restarts() {
        let (_dir, endpoint) = temp_endpoint();
        let listener = endpoint.bind().expect("bind");
        // First daemon answers one request, then hangs up (a restart).
        let first = serve(listener, vec!["{\"ok\":true}\n"]);
        let link = DaemonLink::new(endpoint.clone());
        link.call(Request::Next).await.expect("first daemon");
        assert_eq!(first.await.expect("first server"), 1);

        // The listener above is dropped (interprocess unlinks its socket file); a new
        // daemon binds the same path.
        let _ = std::fs::remove_file(endpoint.path());
        let listener = endpoint.bind().expect("rebind");
        let second = serve(listener, vec!["{\"ok\":true}\n"]);
        link.call(Request::Next)
            .await
            .expect("resent on a new connection");
        assert_eq!(second.await.expect("second server"), 1);
    }

    #[tokio::test]
    async fn ok_false_is_a_daemon_error() {
        let (_dir, endpoint) = temp_endpoint();
        let listener = endpoint.bind().expect("bind");
        let _server = serve(listener, vec!["{\"ok\":false,\"error\":\"no history\"}\n"]);
        let link = DaemonLink::new(endpoint);

        let err = link.call(Request::Prev).await.expect_err("ok:false");
        assert!(
            matches!(&err, LinkError::Daemon(msg) if msg == "no history"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn status_decodes_the_payload() {
        let (_dir, endpoint) = temp_endpoint();
        let listener = endpoint.bind().expect("bind");
        let _server = serve(
            listener,
            vec![concat!(
                "{\"ok\":true,\"data\":{\"current\":null,\"paused\":true,\"interval\":\"30m\",",
                "\"theme\":\"dark\",\"history_count\":0,\"history_index\":-1}}\n"
            )],
        );
        let link = DaemonLink::new(endpoint);

        let status = link.status().await.expect("status");
        assert!(status.paused);
        assert_eq!(status.interval, "30m");
    }

    #[tokio::test]
    async fn missing_daemon_is_not_running() {
        let (_dir, endpoint) = temp_endpoint();
        let link = DaemonLink::new(endpoint);

        let err = link.call(Request::Status).await.expect_err("no daemon");
        assert!(err.is_not_running(), "{err:?}");
    }
}

#[cfg(test)]
mod socket_tests {
    use std::path::{Path, PathBuf};

    use kakemono_config::paths::{BaseDirs, Os, Paths, legacy_default_socket};

    use super::socket_override;

    /// A machine whose home is a temp dir, with no `$KAKEMONO_CONFIG`.
    fn paths_in(home: &Path) -> Paths {
        let base = BaseDirs {
            home: home.to_owned(),
            config_home: home.join(".config"),
            cache_home: home.join(".cache"),
            state_home: None,
            runtime_home: None,
            config_override: None,
        };
        Paths::from_base(&base, Os::current())
    }

    fn write_config(file: &Path, socket: &Path) {
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        let socket = socket.to_str().expect("utf-8").replace('\\', "\\\\");
        std::fs::write(file, format!("[daemon]\nsocket_path = \"{socket}\"\n")).expect("write");
    }

    #[test]
    fn legacy_config_with_the_go_socket_uses_the_kakemono_default() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(home.path());
        write_config(
            &paths.legacy_config_file,
            &legacy_default_socket(home.path()),
        );

        assert_eq!(socket_override(&paths), None);
    }

    #[test]
    fn legacy_config_with_another_socket_keeps_it() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(home.path());
        let custom = home.path().join("run/custom.sock");
        write_config(&paths.legacy_config_file, &custom);

        assert_eq!(socket_override(&paths), Some(custom));
    }

    #[test]
    fn kakemono_config_socket_is_honoured_even_at_the_go_path() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(home.path());
        let go = legacy_default_socket(home.path());
        write_config(&paths.config_file, &go);
        // The kakemono file wins over the legacy one.
        write_config(&paths.legacy_config_file, Path::new("/ignored.sock"));

        assert_eq!(socket_override(&paths), Some(go));
    }

    #[test]
    fn no_config_means_no_override() {
        let home = tempfile::tempdir().expect("tempdir");
        assert_eq!(socket_override(&paths_in(home.path())), None::<PathBuf>);
    }
}

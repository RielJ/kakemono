//! Async client for the daemon socket (plan §I).
//!
//! One connection carries any number of request/response pairs, one line each, like Go's
//! server loop. [`Client::subscribe`] turns the connection into an event stream.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use futures::Stream;
use interprocess::local_socket::tokio::prelude::*;
use tokio::io::BufReader;

use crate::codec::{self, CodecError};
use crate::endpoint::Endpoint;
use crate::types::{Command, Event, Request, Response};

/// Connect timeout, as Go's `dialTimeout`.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Round-trip timeout for one request, as Go's connection deadline.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Nothing listens on the endpoint (socket missing, connection refused, or the connect
    /// timed out). The message matches Go's `daemonNotRunning` hint.
    #[error(
        "kakemono daemon is not running.\nStart it with:  kakemono daemon\n\
         (original error: cannot connect to daemon socket {:?}: {source})",
        endpoint.display()
    )]
    DaemonNotRunning {
        endpoint: PathBuf,
        source: io::Error,
    },
    /// Any other connect failure, such as permission denied.
    #[error("cannot connect to daemon socket {:?}: {source}", endpoint.display())]
    Connect {
        endpoint: PathBuf,
        source: io::Error,
    },
    #[error("sending command: {0}")]
    Send(#[source] CodecError),
    #[error("decoding response: {0}")]
    Receive(#[source] CodecError),
    /// The daemon closed the connection before answering.
    #[error("decoding response: daemon closed the connection")]
    Closed,
    #[error("request timed out after {}s", REQUEST_TIMEOUT.as_secs())]
    Timeout,
    /// The daemon answered `subscribe` with `ok:false`, e.g. an older daemon that does not
    /// know the command.
    #[error("subscribe rejected by daemon: {0}")]
    SubscribeRejected(String),
}

impl ClientError {
    fn from_connect(endpoint: &Endpoint, source: io::Error) -> Self {
        let endpoint = endpoint.path().to_path_buf();
        match source.kind() {
            io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::TimedOut => Self::DaemonNotRunning { endpoint, source },
            _ => Self::Connect { endpoint, source },
        }
    }
}

/// A connection to the daemon.
#[derive(Debug)]
pub struct Client {
    // Writes go through `get_mut()`: requests and responses alternate, so the buffer
    // never holds unread bytes when we write.
    conn: BufReader<LocalSocketStream>,
}

impl Client {
    /// Connects within [`CONNECT_TIMEOUT`].
    pub async fn connect(endpoint: &Endpoint) -> Result<Self, ClientError> {
        let name = endpoint
            .name()
            .map_err(|e| ClientError::from_connect(endpoint, e))?;
        let stream = tokio::time::timeout(CONNECT_TIMEOUT, LocalSocketStream::connect(name))
            .await
            .map_err(|_| {
                ClientError::from_connect(endpoint, io::Error::from(io::ErrorKind::TimedOut))
            })?
            .map_err(|e| ClientError::from_connect(endpoint, e))?;
        Ok(Self {
            conn: BufReader::new(stream),
        })
    }

    /// Sends one command and waits up to [`REQUEST_TIMEOUT`] for its response. An
    /// `ok:false` response is returned as `Ok`; only transport failures are errors.
    pub async fn request(&mut self, request: impl Into<Command>) -> Result<Response, ClientError> {
        let command = request.into();
        tokio::time::timeout(REQUEST_TIMEOUT, self.round_trip(&command))
            .await
            .map_err(|_| ClientError::Timeout)?
    }

    async fn round_trip(&mut self, command: &Command) -> Result<Response, ClientError> {
        codec::write_line_json(self.conn.get_mut(), command)
            .await
            .map_err(ClientError::Send)?;
        codec::read_line_json(&mut self.conn)
            .await
            .map_err(ClientError::Receive)?
            .ok_or(ClientError::Closed)
    }

    /// Sends `subscribe`, expects `{"ok":true}`, then yields one item per event line until
    /// the daemon closes the connection. The stream ends after its first error.
    ///
    /// Event names this version does not know arrive as [`Event::Unknown`].
    pub async fn subscribe(
        mut self,
    ) -> Result<impl Stream<Item = Result<Event, ClientError>> + Send + Unpin, ClientError> {
        let ack = self.request(Request::Subscribe).await?;
        if !ack.ok {
            return Err(ClientError::SubscribeRejected(ack.error));
        }
        Ok(Box::pin(futures::stream::unfold(
            Some(self.conn),
            |state| async move {
                let mut conn = state?;
                match codec::read_line_json::<Event, _>(&mut conn).await {
                    Ok(Some(event)) => Some((Ok(event), Some(conn))),
                    Ok(None) => None,
                    Err(e) => Some((Err(ClientError::Receive(e)), None)),
                }
            },
        )))
    }
}

/// Whether something accepts connections on `endpoint`, as Go's `Client.IsRunning`.
pub async fn is_running(endpoint: &Endpoint) -> bool {
    Client::connect(endpoint).await.is_ok()
}

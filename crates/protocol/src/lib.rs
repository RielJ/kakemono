//! IPC contract shared by the daemon, the CLI and the UI: wire types, the newline-JSON codec,
//! default endpoint naming and the async client.

pub mod client;
pub mod codec;
pub mod endpoint;
pub mod types;

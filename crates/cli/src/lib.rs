//! The `wallpaperd` CLI and the headless daemon host. A library so integration tests can drive
//! the commands and the IPC server in-process (plan §A, §C, §I).

pub mod cli;
pub mod commands;
pub mod daemon;
pub mod firstrun;
pub mod logging;
pub mod output;

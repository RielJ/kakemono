//! `kakemono` binary: parse the command line and run it (`kakemono_cli::cli`).

use std::process::ExitCode;

fn main() -> ExitCode {
    kakemono_cli::cli::main_with_args(std::env::args_os())
}

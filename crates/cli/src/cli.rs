//! The clap derive tree, the shared [`Context`] (global `--config` / `--socket`, the
//! IPC endpoint) and the dispatcher [`run`].
//!
//! Client commands (`next`, `prev`, `set`, `favorite`, `trash`, `status`, `pause`, `resume`) run
//! on a current-thread tokio runtime built here. Errors print to stderr as Go did (the message
//! alone) and exit 1.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, anyhow, bail};
use clap::{CommandFactory, Parser, Subcommand};
use kakemono_config::paths::{self, ConfigSource, Paths};
use kakemono_protocol::client::Client;
use kakemono_protocol::endpoint::{self, Endpoint};
use kakemono_protocol::types::{Request, Response, StatusData};

use crate::commands;
use crate::commands::simple::{self, Simple};

#[derive(Debug, Parser)]
#[command(
    name = "kakemono",
    version,
    about = "A desktop wallpaper manager daemon",
    long_about = "kakemono is a desktop wallpaper manager that supports multiple download sources \
                  (Wallhaven, Unsplash, Bing, Reddit, APOD, Local), auto-rotation with light/dark \
                  theme awareness, history/favorites/trash management, and a floating picker.",
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Commands,
}

/// Flags every subcommand accepts.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct GlobalArgs {
    /// Config file path (default: $KAKEMONO_CONFIG, then ~/.config/kakemono/config.toml)
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Daemon socket path (default: config `socket_path`, $KAKEMONO_SOCKET, then the platform
    /// default)
    #[arg(long, global = true, value_name = "PATH")]
    pub socket: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Start the wallpaper daemon
    Daemon {
        /// Run in the foreground (don't detach)
        #[arg(short, long)]
        foreground: bool,
        /// Internal: set by the background re-exec; log to the log file instead of stderr
        #[arg(long, hide = true, requires = "foreground")]
        detached: bool,
    },
    /// Switch to the next wallpaper
    #[command(long_about = "Ask the daemon to advance to the next wallpaper.\n\n\
        If there is a future entry in the rotation history it is restored;\n\
        otherwise a fresh wallpaper is fetched from the configured provider.")]
    Next,
    /// Switch to the previous wallpaper
    #[command(long_about = "Ask the daemon to revert to the previous wallpaper from history.")]
    Prev,
    /// Set a specific image as the wallpaper
    #[command(
        long_about = "Tell the daemon to set a specific local image file as the desktop \
        wallpaper.\n\nThe path is added to the rotation history so that prev/next work as \
        expected.\n\nExample:\n  kakemono set ~/Pictures/mountain.jpg"
    )]
    Set {
        #[arg(value_name = "IMAGE-PATH")]
        path: PathBuf,
    },
    /// Save the current wallpaper to favorites
    #[command(
        long_about = "Copy the current wallpaper to the favorites directory configured in\n\
        config.toml (defaults to ~/Pictures/Wallpapers/favorites)."
    )]
    Favorite,
    /// Trash the current wallpaper and advance to the next
    #[command(
        long_about = "Move the current wallpaper to the trash directory and immediately\n\
        switch to the next one."
    )]
    Trash,
    /// Show daemon and wallpaper status
    #[command(long_about = "Print the current wallpaper and daemon state.\n\n\
        Use --json to get machine-readable output (useful for sketchybar scripts).")]
    Status {
        /// Output raw JSON instead of pretty-printed text
        #[arg(long)]
        json: bool,
    },
    /// Pause automatic wallpaper rotation
    #[command(
        long_about = "Tell the daemon to stop the auto-rotation timer.  Use \"resume\" to \
        restart it."
    )]
    Pause,
    /// Resume automatic wallpaper rotation
    #[command(long_about = "Tell the daemon to restart the auto-rotation timer after a pause.")]
    Resume,
    /// Reveal the current wallpaper, or open the download or favorites folder
    Open {
        /// Open the download folder
        #[arg(long, conflicts_with = "favorites")]
        folder: bool,
        /// Open the favorites folder
        #[arg(long)]
        favorites: bool,
    },
    /// Open the wallpaper picker
    Pick,
    /// Open the settings window
    Settings,
    /// Detect the desktop backend
    Detect {
        /// Save the detected backend to config.toml
        #[arg(long)]
        write: bool,
        /// With --write: overwrite a [desktop] section you manage yourself
        #[arg(long, requires = "write")]
        force: bool,
        /// Internal: detect in a saved session env (JSON) instead of this one
        #[arg(long, hide = true, value_name = "FILE")]
        env_json: Option<PathBuf>,
    },
    /// Diagnose the desktop backend and daemon
    Doctor {
        /// Ask the daemon to re-set the current wallpaper
        #[arg(long)]
        apply: bool,
        /// Internal: check a saved session env (JSON) instead of this one
        #[arg(long, hide = true, value_name = "FILE")]
        env_json: Option<PathBuf>,
    },
    /// Manage the login service that starts the daemon
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Show, create or edit the config file
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Print a shell completion script
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Subcommand)]
pub enum ServiceAction {
    /// Install and start the login service
    Install,
    /// Stop and remove the login service
    Uninstall,
    /// Show whether the login service is installed and running
    Status {
        /// Print {"installed":bool,"running":bool,"path":string|null}
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Subcommand)]
pub enum ConfigAction {
    /// Write the commented default config (refuses to overwrite)
    Init,
    /// Print the config file in use
    Path,
    /// Open the config file in $VISUAL or $EDITOR
    Edit,
}

/// What every command gets: the global flags, and the daemon endpoint resolved from them.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub global: GlobalArgs,
}

impl Context {
    pub fn new(global: GlobalArgs) -> Self {
        Self { global }
    }

    /// The config file in use (`--config`, `$KAKEMONO_CONFIG`, kakemono, or missing).
    pub fn config_source(&self) -> anyhow::Result<(Paths, ConfigSource)> {
        let paths = Paths::detect()?;
        let source = paths.locate_config(self.global.config.as_deref());
        Ok((paths, source))
    }

    /// The daemon endpoint: `--socket`, else the config's `socket_path`
    /// (`Config::socket_override`), else `$KAKEMONO_SOCKET`, else the platform default in
    /// `Paths::runtime_dir` (next to the daemon lock). Same choice as the daemon. A config that
    /// fails to load is reported on stderr and skipped.
    pub fn endpoint(&self) -> anyhow::Result<Endpoint> {
        if let Some(socket) = self
            .global
            .socket
            .as_deref()
            .filter(|s| !s.as_os_str().is_empty())
        {
            return Ok(endpoint::default_endpoint(Some(socket), None)?);
        }
        let (configured, runtime_dir) = match self.load_config() {
            Ok((paths, cfg)) => (
                cfg.socket_override().map(Path::to_path_buf),
                Some(paths.runtime_dir),
            ),
            Err(e) => {
                eprintln!("warning: {e:#}; using the default socket");
                (None, Paths::detect().ok().map(|p| p.runtime_dir))
            }
        };
        Ok(endpoint::default_endpoint(
            configured.as_deref(),
            runtime_dir.as_deref(),
        )?)
    }

    fn load_config(&self) -> anyhow::Result<(Paths, kakemono_config::Config)> {
        let paths = Paths::detect()?;
        let (cfg, _) = paths.load_config(self.global.config.as_deref())?;
        Ok((paths, cfg))
    }

    /// Connects to the daemon. Not running → Go's "daemon is not running" hint.
    pub async fn client(&self) -> anyhow::Result<Client> {
        let endpoint = self.endpoint()?;
        Ok(Client::connect(&endpoint).await?)
    }

    /// IPC `status`, decoded. A missing or malformed payload is `parsing status: ...`, as Go.
    pub async fn status(&self) -> anyhow::Result<StatusData> {
        self.call(Request::Status)
            .await?
            .data_as()
            .context("parsing status")?
            .context("parsing status: no data")
    }

    /// Sends one request on a fresh connection. `ok:false` becomes `daemon error: <message>`.
    pub async fn call(&self, request: Request) -> anyhow::Result<Response> {
        let mut client = self.client().await?;
        let response = client.request(request).await?;
        if !response.ok {
            bail!("daemon error: {}", response.error);
        }
        Ok(response)
    }
}

/// Parses `args` (including argv\[0\]) and runs the command. For `main` and tests.
pub fn main_with_args<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match Cli::try_parse_from(args) {
        Ok(cli) => run(cli),
        Err(e) => {
            let _ = e.print();
            ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(2))
        }
    }
}

/// Exit status when the reader of stdout goes away (`kakemono doctor | head`): 128 + SIGPIPE (13),
/// the status a shell reports for a program such as `cat` or `ls` killed by SIGPIPE.
pub const STDOUT_CLOSED_EXIT: u8 = 128 + 13;

/// Runs a parsed command line. Prints the error (message only, as Go) and returns exit code 1 on
/// failure. A closed stdout exits with [`STDOUT_CLOSED_EXIT`] and prints nothing.
pub fn run(cli: Cli) -> ExitCode {
    let ctx = Context::new(cli.global);
    match dispatch(&ctx, cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if is_stdout_closed(&e) => ExitCode::from(STDOUT_CLOSED_EXIT),
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

/// The payload of the `io::Error` that [`Stdout`] returns when stdout's reader is gone. Only
/// stdout gets it, so a broken pipe to the daemon socket or a hook still reports an error.
#[derive(Debug, thiserror::Error)]
#[error("stdout was closed")]
struct StdoutClosed;

/// Stdout for commands. Marks `BrokenPipe` errors with [`StdoutClosed`]; other errors pass
/// through unchanged.
struct Stdout<W>(W);

impl<W: Write> Write for Stdout<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf).map_err(mark_stdout_closed)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush().map_err(mark_stdout_closed)
    }
}

fn mark_stdout_closed(e: io::Error) -> io::Error {
    if e.kind() == io::ErrorKind::BrokenPipe {
        io::Error::new(io::ErrorKind::BrokenPipe, StdoutClosed)
    } else {
        e
    }
}

/// True when `e`, or an error in its chain, is the `io::Error` from a write to a closed stdout.
fn is_stdout_closed(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        cause.is::<StdoutClosed>()
            || cause
                .downcast_ref::<io::Error>()
                .and_then(io::Error::get_ref)
                .is_some_and(|inner| inner.is::<StdoutClosed>())
    })
}

fn dispatch(ctx: &Context, command: Commands) -> anyhow::Result<()> {
    let mut stdout = Stdout(io::stdout().lock());
    run_command(ctx, &mut stdout, command)?;
    // Text without a final newline is still buffered. Write it now so its error is reported.
    stdout.flush()?;
    Ok(())
}

fn run_command(ctx: &Context, out: &mut dyn Write, command: Commands) -> anyhow::Result<()> {
    match command {
        Commands::Next => block_on(simple::run(ctx, out, Simple::Next)),
        Commands::Prev => block_on(simple::run(ctx, out, Simple::Prev)),
        Commands::Set { path } => block_on(commands::set::run(ctx, out, &path)),
        Commands::Favorite => block_on(simple::run(ctx, out, Simple::Favorite)),
        Commands::Trash => block_on(simple::run(ctx, out, Simple::Trash)),
        Commands::Status { json } => block_on(simple::status(ctx, out, json)),
        Commands::Pause => block_on(simple::run(ctx, out, Simple::Pause)),
        Commands::Resume => block_on(simple::run(ctx, out, Simple::Resume)),
        Commands::Completions { shell } => {
            // `generate` panics when a write fails, so render to memory first.
            let mut script = Vec::new();
            clap_complete::generate(shell, &mut Cli::command(), "kakemono", &mut script);
            out.write_all(&script)?;
            Ok(())
        }
        Commands::Config { action } => config_command(ctx, out, action),
        Commands::Daemon {
            foreground,
            detached,
        } => commands::daemon::run(ctx, out, foreground, detached),
        Commands::Open { folder, favorites } => {
            block_on(commands::open::run(ctx, out, folder, favorites))
        }
        Commands::Pick => commands::pick::run(ctx, out),
        Commands::Settings => commands::settings::run(ctx, out),
        Commands::Detect {
            write,
            force,
            env_json,
        } => block_on(commands::detect::run(
            ctx,
            out,
            write,
            force,
            env_json.as_deref(),
        )),
        Commands::Doctor { apply, env_json } => {
            block_on(commands::doctor::run(ctx, out, apply, env_json.as_deref()))
        }
        Commands::Service { action } => commands::service::run(ctx, out, action),
    }
}

fn block_on<F: Future<Output = anyhow::Result<()>>>(future: F) -> anyhow::Result<()> {
    runtime()?.block_on(future)
}

/// The current-thread runtime every client command runs on.
pub(crate) fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")
}

fn config_command(ctx: &Context, out: &mut dyn Write, action: ConfigAction) -> anyhow::Result<()> {
    let (_, source) = ctx.config_source()?;
    match action {
        ConfigAction::Path => {
            writeln!(out, "{}", source.path().display())?;
        }
        ConfigAction::Init => {
            let target = source.path();
            paths::init(target)?;
            writeln!(out, "Wrote {}", target.display())?;
        }
        ConfigAction::Edit => {
            let path = source.path();
            if !path.exists() {
                paths::init(path)?;
            }
            let editor = std::env::var_os("VISUAL")
                .filter(|v| !v.is_empty())
                .or_else(|| std::env::var_os("EDITOR").filter(|v| !v.is_empty()))
                .ok_or_else(|| anyhow!("set $VISUAL or $EDITOR to edit {}", path.display()))?;
            let editor = editor.to_string_lossy().into_owned();
            let mut argv = shell_split(&editor)?;
            if argv.is_empty() {
                bail!("$VISUAL/$EDITOR is empty");
            }
            let program = argv.remove(0);
            let status = std::process::Command::new(&program)
                .args(argv)
                .arg(path)
                .status()
                .with_context(|| format!("running {program}"))?;
            if !status.success() {
                bail!("{program} exited with {status}");
            }
        }
    }
    Ok(())
}

/// `$EDITOR` may carry flags (`code -w`) and a quoted path with spaces; split it with shell
/// quoting rules, as git does.
fn shell_split(s: &str) -> anyhow::Result<Vec<String>> {
    shell_words::split(s).with_context(|| format!("cannot parse $VISUAL/$EDITOR: {s}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `$EDITOR` is split like a shell would: a quoted path with spaces stays one word.
    #[test]
    fn editor_is_split_like_a_shell() {
        assert_eq!(
            shell_split("'/Applications/Sublime Text.app/bin/subl' -w").unwrap(),
            ["/Applications/Sublime Text.app/bin/subl", "-w"]
        );
        assert_eq!(shell_split("code --wait").unwrap(), ["code", "--wait"]);
        assert!(shell_split("vim 'unclosed").is_err());
    }

    #[test]
    fn clap_tree_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_work_after_the_subcommand() {
        let cli =
            Cli::try_parse_from(["kakemono", "status", "--json", "--socket", "/tmp/s"]).unwrap();
        assert_eq!(cli.global.socket.as_deref(), Some(Path::new("/tmp/s")));
        assert!(matches!(cli.command, Commands::Status { json: true }));
    }

    #[test]
    fn daemon_short_foreground_flag() {
        let cli = Cli::try_parse_from(["kakemono", "daemon", "-f"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Daemon {
                foreground: true,
                detached: false
            }
        ));
    }

    #[test]
    fn set_needs_exactly_one_path() {
        assert!(Cli::try_parse_from(["kakemono", "set"]).is_err());
        assert!(Cli::try_parse_from(["kakemono", "set", "a", "b"]).is_err());
    }

    #[test]
    fn detect_force_requires_write() {
        assert!(Cli::try_parse_from(["kakemono", "detect", "--force"]).is_err());
        assert!(Cli::try_parse_from(["kakemono", "detect", "--write", "--force"]).is_ok());
    }
}

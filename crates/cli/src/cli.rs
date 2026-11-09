//! The clap derive tree (plan §I), the shared [`Context`] (global `--config` / `--socket`, the
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
use kakemono_protocol::types::{Request, Response};

use crate::commands;

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
    /// Config file path (default: $KAKEMONO_CONFIG, ~/.config/kakemono/config.toml, then the
    /// legacy ~/.config/wallpaperd/config.toml)
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
    },
    /// Diagnose the desktop backend and daemon
    Doctor {
        /// Re-set the current wallpaper after the checks
        #[arg(long)]
        apply: bool,
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
    Status,
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

    /// The config file in use (`--config`, `$KAKEMONO_CONFIG`, kakemono, legacy, or missing).
    pub fn config_source(&self) -> anyhow::Result<(Paths, ConfigSource)> {
        let paths = Paths::detect()?;
        let source = paths.locate_config(self.global.config.as_deref());
        Ok((paths, source))
    }

    /// The daemon endpoint: `--socket`, else the config's `socket_path` (through
    /// `Config::effective_socket_path`, so the legacy Go socket is ignored), else
    /// `$KAKEMONO_SOCKET`, else the platform default. Same choice as the daemon. A config that
    /// fails to load is reported on stderr and skipped.
    pub fn endpoint(&self) -> anyhow::Result<Endpoint> {
        if let Some(socket) = self
            .global
            .socket
            .as_deref()
            .filter(|s| !s.as_os_str().is_empty())
        {
            return Ok(endpoint::default_endpoint(Some(socket))?);
        }
        let configured = match self.load_config() {
            Ok((paths, cfg, source)) => {
                let socket = cfg.effective_socket_path(&source, &paths.home);
                if let Some(notice) = socket.notice() {
                    tracing::info!("{notice}");
                }
                socket.override_path().map(Path::to_path_buf)
            }
            Err(e) => {
                eprintln!("warning: {e:#}; using the default socket");
                None
            }
        };
        Ok(endpoint::default_endpoint(configured.as_deref())?)
    }

    fn load_config(&self) -> anyhow::Result<(Paths, kakemono_config::Config, ConfigSource)> {
        let paths = Paths::detect()?;
        let (cfg, source) = paths.load_config(self.global.config.as_deref())?;
        Ok((paths, cfg, source))
    }

    /// Connects to the daemon. Not running → Go's "daemon is not running" hint.
    pub async fn client(&self) -> anyhow::Result<Client> {
        let endpoint = self.endpoint()?;
        Ok(Client::connect(&endpoint).await?)
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

/// Runs a parsed command line. Prints the error (message only, as Go) and returns exit code 1 on
/// failure.
pub fn run(cli: Cli) -> ExitCode {
    let ctx = Context::new(cli.global);
    match dispatch(&ctx, cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(ctx: &Context, command: Commands) -> anyhow::Result<()> {
    let mut stdout = io::stdout().lock();
    let out: &mut dyn Write = &mut stdout;
    match command {
        Commands::Next => block_on(commands::next::run(ctx, out)),
        Commands::Prev => block_on(commands::prev::run(ctx, out)),
        Commands::Set { path } => block_on(commands::set::run(ctx, out, &path)),
        Commands::Favorite => block_on(commands::favorite::run(ctx, out)),
        Commands::Trash => block_on(commands::trash::run(ctx, out)),
        Commands::Status { json } => block_on(commands::status::run(ctx, out, json)),
        Commands::Pause => block_on(commands::pause::run(ctx, out)),
        Commands::Resume => block_on(commands::resume::run(ctx, out)),
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "kakemono", out);
            Ok(())
        }
        Commands::Config { action } => config_command(ctx, out, action),
        // Wired by later tasks; each replaces its arm.
        Commands::Daemon {
            foreground,
            detached,
        } => commands::daemon::run(ctx, out, foreground, detached),
        Commands::Open { folder, favorites } => {
            block_on(commands::open::run(ctx, out, folder, favorites))
        }
        Commands::Pick => commands::pick::run(ctx, out),
        Commands::Settings => commands::settings::run(ctx, out),
        Commands::Detect { .. } => not_available("detect"), // Task 37
        Commands::Doctor { .. } => not_available("doctor"), // Task 37
        Commands::Service { .. } => not_available("service"), // Task 39
    }
}

fn not_available(name: &str) -> anyhow::Result<()> {
    Err(anyhow!(
        "`kakemono {name}` is not available in this build yet"
    ))
}

fn block_on<F: Future<Output = anyhow::Result<()>>>(future: F) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?
        .block_on(future)
}

fn config_command(ctx: &Context, out: &mut dyn Write, action: ConfigAction) -> anyhow::Result<()> {
    let (_, source) = ctx.config_source()?;
    match action {
        ConfigAction::Path => {
            writeln!(out, "{}", source.path().display())?;
            if source.is_legacy() {
                eprintln!(
                    "note: this is the legacy wallpaperd config; kakemono reads it until {} exists",
                    paths::config_file()?.display()
                );
            }
        }
        ConfigAction::Init => {
            // A legacy config is in use but kakemono's own file is what `init` creates.
            let target = match &source {
                ConfigSource::Legacy(_) => paths::config_file()?,
                other => other.path().to_owned(),
            };
            paths::init(&target)?;
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
            let mut argv = shell_split(&editor);
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

/// `$EDITOR` may carry flags (`code -w`); split on whitespace like most tools do.
fn shell_split(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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

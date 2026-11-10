//! Linux `SystemSignals`.
//!
//! - **Theme.** `[desktop] theme` when it is `light` or `dark` (the same rule as macOS and
//!   Windows). Otherwise the XDG Settings portal's `org.freedesktop.appearance color-scheme`
//!   through dark-light, followed live with `dark_light::stream()`. Without a portal (bare
//!   window managers), one `gsettings get org.gnome.desktop.interface color-scheme` read, and
//!   then light. Only the portal reports changes. → [`Signal::ThemeChanged`], deduplicated.
//! - **Resume.** logind's `org.freedesktop.login1.Manager.PrepareForSleep(false)` on the system
//!   bus → [`Signal::Resumed`].
//!
//! Nothing here fails: a missing portal, logind or system bus logs a warning and drops that
//! signal source.

use std::fmt;
use std::future::Future;
use std::sync::{Arc, Weak};
use std::time::Duration;

use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use kakemono_config::schema::{DesktopConfig, ThemeOverride};
use tokio::sync::{broadcast, oneshot};
use zbus::{MatchRule, MessageStream};

use crate::api::{Signal, SystemSignals, Theme};
use crate::exec::{CommandRunner, Invocation, TokioRunner};
use crate::hub::{SignalHub, mode_theme};
use crate::linux::backends::command::ThemeSource;

/// How long the first portal read may take (dark-light's D-Bus calls are blocking; the portal
/// can be slow to activate).
pub const PORTAL_TIMEOUT: Duration = Duration::from_secs(5);

/// How long connecting to the system bus may take.
pub const SYSTEM_BUS_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the `gsettings` fallback read may take.
pub const GSETTINGS_TIMEOUT: Duration = Duration::from_secs(3);

/// The fallback read when there is no Settings portal.
pub const COLOR_SCHEME_ARGV: [&str; 4] = [
    "gsettings",
    "get",
    "org.gnome.desktop.interface",
    "color-scheme",
];

/// logind's object path.
pub const LOGIN1_PATH: &str = "/org/freedesktop/login1";
/// logind's manager interface.
pub const LOGIN1_MANAGER: &str = "org.freedesktop.login1.Manager";
/// The signal sent before sleep (`true`) and after resume (`false`).
pub const PREPARE_FOR_SLEEP: &str = "PrepareForSleep";

/// Where the theme came from. For `doctor` and logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeOrigin {
    /// `[desktop] theme = "light" | "dark"`. Never changes.
    Override,
    /// The XDG Settings portal. Followed live.
    Portal,
    /// `gsettings get org.gnome.desktop.interface color-scheme`, read once.
    Gsettings,
    /// Nothing answered: light.
    Default,
}

impl ThemeOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeOrigin::Override => "override",
            ThemeOrigin::Portal => "portal",
            ThemeOrigin::Gsettings => "gsettings",
            ThemeOrigin::Default => "default",
        }
    }
}

impl fmt::Display for ThemeOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A portal reading as a theme. `Unspecified` (`color-scheme` 0, "no preference") is light.
pub fn theme_from_mode(mode: dark_light::Mode) -> Theme {
    mode_theme(mode).unwrap_or(Theme::Light)
}

/// Parse `gsettings get org.gnome.desktop.interface color-scheme` output (`'prefer-dark'`,
/// `'prefer-light'` or `'default'`). `None` for anything else.
pub fn parse_color_scheme(stdout: &str) -> Option<Theme> {
    match stdout.trim().trim_matches('\'') {
        "prefer-dark" => Some(Theme::Dark),
        "prefer-light" | "default" => Some(Theme::Light),
        _ => None,
    }
}

/// Read [`COLOR_SCHEME_ARGV`] through `runner`. `None` if it fails or prints something else.
pub async fn read_gsettings_theme(runner: &dyn CommandRunner) -> Option<Theme> {
    let invocation =
        Invocation::new(COLOR_SCHEME_ARGV.map(str::to_owned).to_vec()).timeout(GSETTINGS_TIMEOUT);
    match runner.run(&invocation).await {
        Ok(output) if output.status.success() => parse_color_scheme(&output.stdout),
        Ok(output) => {
            tracing::debug!(status = %output.status, "gsettings color-scheme read failed");
            None
        }
        Err(err) => {
            tracing::debug!(%err, "gsettings is not available for the theme");
            None
        }
    }
}

/// Run a blocking dark-light call on a plain thread (dark-light's zbus 5 blocking API starts its
/// own runtime, which must not happen on a tokio thread). `None` on timeout or a lost thread.
async fn off_runtime<T: Send + 'static>(
    what: &'static str,
    timeout: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("kakemono-theme".to_owned())
        .spawn(move || {
            let _ = tx.send(f());
        });
    if let Err(err) = spawned {
        tracing::warn!(%err, "could not start the {what} thread");
        return None;
    }
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(_)) => None,
        Err(_) => {
            tracing::warn!("{what} did not answer within {timeout:?}");
            None
        }
    }
}

/// The portal's current theme. `None` if there is no Settings portal (or it timed out).
pub async fn read_portal_theme() -> Option<Theme> {
    match off_runtime("Settings portal", PORTAL_TIMEOUT, dark_light::detect).await? {
        Ok(mode) => Some(theme_from_mode(mode)),
        Err(err) => {
            tracing::debug!(%err, "no Settings portal for the theme");
            None
        }
    }
}

/// The portal's theme changes. `None` if there is no Settings portal.
pub async fn portal_theme_stream() -> Option<impl Stream<Item = Theme> + Send + Unpin + 'static> {
    match off_runtime("Settings portal", PORTAL_TIMEOUT, dark_light::stream).await? {
        Ok(modes) => Some(modes.map(theme_from_mode)),
        Err(err) => {
            tracing::warn!(%err, "theme changes from the Settings portal will not be followed");
            None
        }
    }
}

/// The starting theme, in order: the override, the portal, `gsettings`, light. Later sources are
/// only polled when the earlier ones have no answer.
pub async fn initial_theme(
    theme: ThemeOverride,
    portal: impl Future<Output = Option<Theme>>,
    gsettings: impl Future<Output = Option<Theme>>,
) -> (Theme, ThemeOrigin) {
    if let Some(theme) = Theme::from_override(theme) {
        return (theme, ThemeOrigin::Override);
    }
    if let Some(theme) = portal.await {
        return (theme, ThemeOrigin::Portal);
    }
    if let Some(theme) = gsettings.await {
        return (theme, ThemeOrigin::Gsettings);
    }
    (Theme::Light, ThemeOrigin::Default)
}

/// The match rule for logind's `PrepareForSleep`. No sender: zbus matches senders client-side
/// against unique names only, and a spoofed signal only causes a re-apply.
#[allow(
    clippy::expect_used,
    reason = "every field is a valid constant, so `build` cannot fail"
)]
pub fn prepare_for_sleep_rule() -> MatchRule<'static> {
    MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(LOGIN1_PATH)
        .interface(LOGIN1_MANAGER)
        .member(PREPARE_FOR_SLEEP)
        .build()
        .expect("constant, valid match rule")
}

/// One item per resume (`PrepareForSleep(false)`) seen on `conn`. Ends when `conn` closes.
pub async fn resume_events(conn: &zbus::Connection) -> zbus::Result<BoxStream<'static, ()>> {
    let messages = MessageStream::for_match_rule(prepare_for_sleep_rule(), conn, None).await?;
    Ok(messages
        .filter_map(|msg| async move {
            let msg = msg.ok()?;
            let start: bool = msg.body().deserialize().ok()?;
            (!start).then_some(())
        })
        .boxed())
}

struct Shared {
    hub: SignalHub,
    origin: ThemeOrigin,
    screen: crate::screen::ScreenCache,
}

/// Linux signals. Cheap to share through `Arc`; the watcher tasks stop after the last
/// `LinuxSignals` is dropped (at their next event).
pub struct LinuxSignals {
    shared: Arc<Shared>,
}

impl fmt::Debug for LinuxSignals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinuxSignals")
            .field("theme", &self.shared.hub.theme())
            .field("origin", &self.shared.origin)
            .finish_non_exhaustive()
    }
}

impl LinuxSignals {
    /// Read the theme, follow the portal and logind. Must run inside the tokio runtime. Never
    /// fails; missing sources are logged and skipped.
    pub async fn start(cfg: &DesktopConfig) -> Self {
        let (theme, origin) = initial_theme(cfg.theme, read_portal_theme(), async {
            read_gsettings_theme(&TokioRunner).await
        })
        .await;
        tracing::info!(%theme, %origin, "system theme");
        let signals = Self::new(theme, origin);
        if origin == ThemeOrigin::Portal
            && let Some(changes) = portal_theme_stream().await
        {
            signals.follow_theme(changes);
        }
        let followed = tokio::time::timeout(SYSTEM_BUS_TIMEOUT, async {
            let conn = zbus::Connection::system().await?;
            signals.follow_resume(conn).await
        })
        .await;
        match followed {
            Ok(Ok(_task)) => tracing::debug!("watching logind for resume"),
            Ok(Err(err)) => tracing::warn!(%err, "cannot watch logind; resume will not be noticed"),
            Err(_) => tracing::warn!("the system bus did not answer; resume will not be noticed"),
        }
        signals
    }

    /// Signals with a fixed theme and nothing attached.
    pub fn new(theme: Theme, origin: ThemeOrigin) -> Self {
        Self {
            shared: Arc::new(Shared {
                hub: SignalHub::new(theme),
                origin,
                screen: crate::screen::ScreenCache::default(),
            }),
        }
    }

    /// Where the starting theme came from.
    pub fn origin(&self) -> ThemeOrigin {
        self.shared.origin
    }

    /// The current theme as a `{theme}` source for command setters.
    pub fn theme_source(&self) -> ThemeSource {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || shared.hub.theme())
    }

    /// Feed theme readings into these signals (deduplicated). The task ends with the stream or
    /// after these signals are dropped.
    pub fn follow_theme(
        &self,
        mut themes: impl Stream<Item = Theme> + Send + Unpin + 'static,
    ) -> tokio::task::JoinHandle<()> {
        let shared: Weak<Shared> = Arc::downgrade(&self.shared);
        tokio::spawn(async move {
            while let Some(theme) = themes.next().await {
                let Some(shared) = shared.upgrade() else {
                    break;
                };
                shared.hub.update_theme(theme);
            }
            tracing::debug!("theme stream ended");
        })
    }

    /// Emit [`Signal::Resumed`] for each logind `PrepareForSleep(false)` on `conn`. Subscribed
    /// when this returns; the task ends with `conn` or after these signals are dropped.
    pub async fn follow_resume(
        &self,
        conn: zbus::Connection,
    ) -> zbus::Result<tokio::task::JoinHandle<()>> {
        let shared: Weak<Shared> = Arc::downgrade(&self.shared);
        let mut resumes = resume_events(&conn).await?;
        Ok(tokio::spawn(async move {
            // Keep the connection open for as long as the stream is read.
            let _conn = conn;
            while resumes.next().await.is_some() {
                let Some(shared) = shared.upgrade() else {
                    break;
                };
                tracing::info!("resumed from sleep");
                shared.hub.send(Signal::Resumed);
            }
            tracing::debug!("logind signal stream ended");
        }))
    }
}

impl SystemSignals for LinuxSignals {
    fn theme(&self) -> Theme {
        self.shared.hub.theme()
    }

    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.shared.hub.subscribe()
    }

    /// The largest connected DRM output's preferred mode ([`crate::screen`]).
    fn screen_size(&self) -> Option<(u32, u32)> {
        use crate::screen::{TTL, drm_sizes, largest};
        self.shared
            .screen
            .get_with(std::time::Instant::now(), TTL, || {
                largest(drm_sizes(std::path::Path::new("/sys/class/drm")))
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::api::CommandStatus;
    use crate::exec::{Output, RunError};

    #[test]
    fn color_scheme_parsing() {
        assert_eq!(parse_color_scheme("'prefer-dark'\n"), Some(Theme::Dark));
        assert_eq!(parse_color_scheme("'prefer-light'\n"), Some(Theme::Light));
        assert_eq!(parse_color_scheme("'default'\n"), Some(Theme::Light));
        assert_eq!(parse_color_scheme(""), None);
        assert_eq!(parse_color_scheme("No such key “color-scheme”"), None);
    }

    #[test]
    fn portal_modes() {
        assert_eq!(theme_from_mode(dark_light::Mode::Dark), Theme::Dark);
        assert_eq!(theme_from_mode(dark_light::Mode::Light), Theme::Light);
        assert_eq!(theme_from_mode(dark_light::Mode::Unspecified), Theme::Light);
    }

    async fn never() -> Option<Theme> {
        panic!("a later theme source was polled after an earlier one answered")
    }

    #[tokio::test]
    async fn initial_theme_order() {
        let dark = || async { Some(Theme::Dark) };
        let none = || async { None };
        assert_eq!(
            initial_theme(ThemeOverride::Light, never(), never()).await,
            (Theme::Light, ThemeOrigin::Override)
        );
        assert_eq!(
            initial_theme(ThemeOverride::Auto, dark(), never()).await,
            (Theme::Dark, ThemeOrigin::Portal)
        );
        assert_eq!(
            initial_theme(ThemeOverride::Auto, none(), dark()).await,
            (Theme::Dark, ThemeOrigin::Gsettings)
        );
        assert_eq!(
            initial_theme(ThemeOverride::Auto, none(), none()).await,
            (Theme::Light, ThemeOrigin::Default)
        );
    }

    #[derive(Default)]
    struct FakeRunner {
        reply: Mutex<Option<Result<Output, RunError>>>,
        argv: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl CommandRunner for FakeRunner {
        async fn run(&self, invocation: &Invocation) -> Result<Output, RunError> {
            *self.argv.lock().unwrap() = invocation.argv.clone();
            self.reply.lock().unwrap().take().expect("one run")
        }

        fn spawn_detached(&self, _argv: &[String]) -> Result<u32, RunError> {
            unreachable!()
        }
    }

    fn output(code: i32, stdout: &str) -> Output {
        Output {
            status: CommandStatus::Exited(code),
            stdout: stdout.to_owned(),
            stderr: String::new(),
        }
    }

    #[tokio::test]
    async fn gsettings_fallback_reads_color_scheme() {
        let runner = FakeRunner::default();
        *runner.reply.lock().unwrap() = Some(Ok(output(0, "'prefer-dark'\n")));
        assert_eq!(read_gsettings_theme(&runner).await, Some(Theme::Dark));
        assert_eq!(*runner.argv.lock().unwrap(), COLOR_SCHEME_ARGV);

        *runner.reply.lock().unwrap() = Some(Ok(output(1, "")));
        assert_eq!(read_gsettings_theme(&runner).await, None);

        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "no gsettings");
        *runner.reply.lock().unwrap() = Some(Err(RunError::Spawn {
            argv0: "gsettings".to_owned(),
            source: missing,
        }));
        assert_eq!(read_gsettings_theme(&runner).await, None);
    }

    #[tokio::test]
    async fn theme_changes_are_deduplicated() {
        let signals = LinuxSignals::new(Theme::Light, ThemeOrigin::Portal);
        let mut rx = signals.subscribe();
        let (tx, themes) = futures::channel::mpsc::unbounded();
        let task = signals.follow_theme(themes);
        for theme in [Theme::Light, Theme::Dark, Theme::Dark, Theme::Light] {
            tx.unbounded_send(theme).unwrap();
        }
        drop(tx);
        task.await.unwrap();
        assert_eq!(rx.try_recv().unwrap(), Signal::ThemeChanged(Theme::Dark));
        assert_eq!(rx.try_recv().unwrap(), Signal::ThemeChanged(Theme::Light));
        assert!(rx.try_recv().is_err());
        assert_eq!(signals.theme(), Theme::Light);
    }

    /// A client connection and a server peer on a private socket pair (no bus involved).
    async fn p2p() -> (zbus::Connection, zbus::Connection) {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let server = zbus::connection::Builder::unix_stream(a)
            .server(zbus::Guid::generate())
            .p2p()
            .build();
        let client = zbus::connection::Builder::unix_stream(b).p2p().build();
        let (server, client) = tokio::join!(server, client);
        (client.unwrap(), server.unwrap())
    }

    async fn emit(server: &zbus::Connection, interface: &str, member: &str, start: bool) {
        server
            .emit_signal(None::<&str>, LOGIN1_PATH, interface, member, &(start,))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn resume_is_prepare_for_sleep_false_only() {
        let (client, server) = p2p().await;
        let signals = LinuxSignals::new(Theme::Light, ThemeOrigin::Default);
        let mut rx = signals.subscribe();
        let _task = signals.follow_resume(client).await.unwrap();

        emit(&server, LOGIN1_MANAGER, PREPARE_FOR_SLEEP, true).await;
        emit(&server, LOGIN1_MANAGER, "SessionNew", false).await;
        emit(&server, "org.example.Other", PREPARE_FOR_SLEEP, false).await;
        emit(&server, LOGIN1_MANAGER, PREPARE_FOR_SLEEP, false).await;

        let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a Resumed signal")
            .unwrap();
        assert_eq!(got, Signal::Resumed);
        // Exactly one: the sleep, the other member and the other interface are ignored. Signals
        // are dispatched in order, so a spurious one would already be queued or in flight.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn theme_source_follows_the_signals() {
        let signals = LinuxSignals::new(Theme::Dark, ThemeOrigin::Override);
        let source = signals.theme_source();
        assert_eq!(source(), Theme::Dark);
        signals.shared.hub.update_theme(Theme::Light);
        assert_eq!(source(), Theme::Light);
        assert_eq!(signals.origin(), ThemeOrigin::Override);
    }
}

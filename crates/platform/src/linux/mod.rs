//! Linux platform (plan §E): detection ([`detect`]), backends ([`backends`]), signals
//! ([`theme`]), the self-healing [`ResilientSetter`] and [`open_platform`].
//!
//! `detect` compiles everywhere (pure resolver). `backends` and [`ResilientSetter`] compile on
//! every Unix so their tests also run on macOS. `theme`, the D-Bus backends and
//! [`open_platform`] are Linux-only.

pub mod detect;

#[cfg(unix)]
pub mod backends;
#[cfg(target_os = "linux")]
pub mod theme;

#[cfg(unix)]
pub use resilient::{Active, Backends, REDETECT_AFTER, ResilientSetter, same_backend};

#[cfg(target_os = "linux")]
pub use live::{LiveBackends, open_platform, open_platform_in, open_setter};

/// [`ResilientSetter`]: re-detect in memory after repeated `SetError`s (plan §E, "Failure,
/// fallback and re-detection").
#[cfg(unix)]
mod resilient {
    use std::fmt;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use kakemono_config::schema::{DesktopConfig, ManagedBy};

    use crate::api::{
        BackendInfo, Fit, SetError, UNCONFIGURED_HINT, UnconfiguredSetter, WallpaperSetter,
    };
    use crate::linux::backends::persistent::Supervisor;
    use crate::linux::detect::Resolution;

    /// Consecutive `SetError`s before re-detecting.
    pub const REDETECT_AFTER: u32 = 2;

    /// A built backend: the setter for `resolution`.
    #[derive(Clone)]
    pub struct Active {
        pub resolution: Resolution,
        pub setter: Arc<dyn WallpaperSetter>,
        /// The supervisor behind a persistent backend. Stopped when a re-detection switches to
        /// a non-persistent backend.
        pub supervisor: Option<Arc<Supervisor>>,
    }

    impl fmt::Debug for Active {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Active")
                .field("backend", &self.resolution.backend)
                .field("setter", &self.setter.info())
                .field("persistent", &self.supervisor.is_some())
                .finish()
        }
    }

    impl Active {
        /// A backend without a supervisor.
        pub fn new(resolution: Resolution, setter: Arc<dyn WallpaperSetter>) -> Self {
            Self {
                resolution,
                setter,
                supervisor: None,
            }
        }

        /// `resolution` that cannot set anything: every `set` fails with `hint`.
        pub fn unconfigured(resolution: Resolution, hint: impl Into<String>) -> Self {
            Self::new(resolution, Arc::new(UnconfiguredSetter::new(hint)))
        }
    }

    /// Detection and construction, for [`ResilientSetter`]'s re-detection.
    #[async_trait::async_trait]
    pub trait Backends: Send + Sync + 'static {
        /// Detect the backend again, in memory, ignoring the saved `[desktop] backend`.
        async fn redetect(&self) -> Resolution;
        /// The setter for `resolution`.
        async fn build(&self, resolution: &Resolution) -> Result<Active, SetError>;
        /// Resolve `desktop` the way startup does (a `reload` with a changed `[desktop]`).
        async fn resolve(&self, desktop: &DesktopConfig) -> Resolution;
        /// Re-detect with `desktop`'s settings from now on (after a successful rebuild).
        fn reconfigure(&self, desktop: &DesktopConfig);
    }

    /// True if `a` and `b` would run the same backend the same way.
    pub fn same_backend(a: &Resolution, b: &Resolution) -> bool {
        a.backend == b.backend && a.set_command == b.set_command && a.persistent == b.persistent
    }

    /// Wraps the active backend. After [`REDETECT_AFTER`] consecutive failures (and only with
    /// `backends`, i.e. `managed_by = "detect"`) it re-detects in memory. If that finds a
    /// different backend, it retries the same `set` once with it and keeps it on success
    /// (logging a `kakemono detect --write` suggestion). Otherwise the old backend stays and
    /// the original error is returned. Nothing is written to the config.
    ///
    /// With `backends` it can also [`rebuild`](WallpaperSetter::rebuild) itself for a changed
    /// `[desktop]` (Task 37b), whether or not it re-detects.
    pub struct ResilientSetter {
        current: Mutex<Arc<Active>>,
        failures: AtomicU32,
        backends: Option<Arc<dyn Backends>>,
        /// Re-detect after repeated failures (`managed_by = "detect"`); needs `backends`.
        redetect: bool,
        /// One re-detection (or rebuild) at a time.
        switching: tokio::sync::Mutex<()>,
    }

    impl fmt::Debug for ResilientSetter {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("ResilientSetter")
                .field("active", &self.active())
                .field("failures", &self.failures.load(Ordering::SeqCst))
                .field("redetects", &(self.redetect && self.backends.is_some()))
                .finish_non_exhaustive()
        }
    }

    impl ResilientSetter {
        /// Re-detects with `backends`; `None` turns re-detection (and rebuilding) off.
        pub fn new(active: Active, backends: Option<Arc<dyn Backends>>) -> Self {
            let redetect = backends.is_some();
            Self {
                current: Mutex::new(Arc::new(active)),
                failures: AtomicU32::new(0),
                backends,
                redetect,
                switching: tokio::sync::Mutex::new(()),
            }
        }

        /// The setter `open_platform` builds: it can always rebuild for a new `[desktop]` and
        /// re-detects only with `managed_by = "detect"`.
        pub fn for_desktop(
            active: Active,
            backends: Arc<dyn Backends>,
            desktop: &DesktopConfig,
        ) -> Self {
            Self {
                redetect: desktop.managed_by == ManagedBy::Detect,
                ..Self::new(active, Some(backends))
            }
        }

        /// Whether repeated failures re-detect.
        pub fn redetects(&self) -> bool {
            self.redetect && self.backends.is_some()
        }

        /// The backend in use.
        pub fn active(&self) -> Arc<Active> {
            Arc::clone(&self.current.lock().unwrap_or_else(|e| e.into_inner()))
        }

        /// The resolution in use (changes after a successful re-detection).
        pub fn resolution(&self) -> Resolution {
            self.active().resolution.clone()
        }

        /// Consecutive failures since the last success or re-detection.
        pub fn failures(&self) -> u32 {
            self.failures.load(Ordering::SeqCst)
        }

        /// Re-detect and, if the backend differs, retry `set` with it. Returns `err` unless the
        /// retry succeeded.
        async fn redetect_and_retry(
            &self,
            backends: &dyn Backends,
            failed: Arc<Active>,
            image: &Path,
            fit: Fit,
            err: SetError,
        ) -> Result<(), SetError> {
            let _switching = self.switching.lock().await;
            if !Arc::ptr_eq(&self.active(), &failed) {
                // Another `set` switched backends while this one waited.
                return Err(err);
            }
            self.failures.store(0, Ordering::SeqCst);
            let old = failed.resolution.backend;
            tracing::warn!(%old, %err, "wallpaper backend failed {REDETECT_AFTER} times in a row; re-detecting");
            let detected = backends.redetect().await;
            if same_backend(&detected, &failed.resolution) {
                tracing::info!(backend = %old, "re-detection found the same backend; keeping it");
                return Err(err);
            }
            let new = detected.backend;
            let candidate = match backends.build(&detected).await {
                Ok(candidate) => candidate,
                Err(build_err) => {
                    tracing::warn!(%new, err = %build_err, "could not start the re-detected backend; keeping {old}");
                    return Err(err);
                }
            };
            if let Err(retry_err) = candidate.setter.set(image, fit).await {
                tracing::warn!(%new, err = %retry_err, "the re-detected backend failed too; keeping {old}");
                return Err(err);
            }
            if candidate.supervisor.is_none()
                && let Some(supervisor) = &failed.supervisor
            {
                // Persistent → non-persistent: the old command would cover the new wallpaper.
                supervisor.stop().await;
            }
            *self.current.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(candidate);
            tracing::warn!(
                %old, %new,
                "switched wallpaper backend for this session; run `kakemono detect --write` to save it"
            );
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl WallpaperSetter for ResilientSetter {
        async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
            let active = self.active();
            let err = match active.setter.set(image, fit).await {
                Ok(()) => {
                    self.failures.store(0, Ordering::SeqCst);
                    return Ok(());
                }
                Err(err) => err,
            };
            let failures = self.failures.fetch_add(1, Ordering::SeqCst) + 1;
            match &self.backends {
                Some(backends) if self.redetect && failures >= REDETECT_AFTER => {
                    self.redetect_and_retry(backends.as_ref(), active, image, fit, err)
                        .await
                }
                _ => Err(err),
            }
        }

        fn info(&self) -> BackendInfo {
            self.active().setter.info()
        }

        /// Resolve and build `desktop`'s backend. On failure nothing changed and this setter
        /// stays. On success a persistent setter is handed over: the next persistent backend
        /// shares the one [`Supervisor`] (its first `set` replaces the old process), and any
        /// other backend gets it stopped here, so `setter.pid` never names a setter nobody
        /// supervises.
        async fn rebuild(
            &self,
            desktop: &DesktopConfig,
        ) -> Result<Option<Arc<dyn WallpaperSetter>>, SetError> {
            let Some(backends) = &self.backends else {
                return Ok(None);
            };
            let _switching = self.switching.lock().await;
            let resolution = backends.resolve(desktop).await;
            if !resolution.is_usable() {
                let hint = resolution.hint.as_deref().unwrap_or(UNCONFIGURED_HINT);
                return Err(SetError::Unconfigured {
                    hint: format!("{} backend: {hint}", resolution.backend),
                });
            }
            let candidate = backends.build(&resolution).await?;
            let old = self.active();
            if candidate.supervisor.is_none()
                && let Some(supervisor) = &old.supervisor
            {
                tracing::info!(
                    old = %old.resolution.backend,
                    new = %resolution.backend,
                    "stopping the persistent setter for the new backend"
                );
                supervisor.stop().await;
            }
            backends.reconfigure(desktop);
            tracing::info!(
                old = %old.resolution.backend,
                new = %resolution.backend,
                reasons = ?resolution.reasons,
                "wallpaper backend rebuilt for the new [desktop]"
            );
            Ok(Some(Arc::new(Self::for_desktop(
                candidate,
                Arc::clone(backends),
                desktop,
            ))))
        }
    }

    #[cfg(test)]
    mod tests {
        use std::collections::VecDeque;
        use std::path::PathBuf;

        use super::*;
        use crate::linux::detect::{BackendKind, Origin};

        fn resolution(backend: BackendKind) -> Resolution {
            Resolution {
                backend,
                set_command: None,
                persistent: backend.is_persistent(),
                origin: Origin::Detected,
                session_key: "test:wayland".to_owned(),
                hint: None,
                reasons: Vec::new(),
            }
        }

        /// Fails while it has failures queued; records successful calls.
        #[derive(Default)]
        struct Scripted {
            name: &'static str,
            fail: Mutex<u32>,
            calls: Mutex<Vec<(PathBuf, Fit)>>,
        }

        impl Scripted {
            fn new(name: &'static str, fail: u32) -> Arc<Self> {
                Arc::new(Self {
                    name,
                    fail: Mutex::new(fail),
                    calls: Mutex::default(),
                })
            }

            fn calls(&self) -> usize {
                self.calls.lock().unwrap().len()
            }
        }

        #[async_trait::async_trait]
        impl WallpaperSetter for Scripted {
            async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
                let mut fail = self.fail.lock().unwrap();
                if *fail > 0 {
                    *fail -= 1;
                    return Err(SetError::Os(format!("{} failed", self.name)));
                }
                self.calls.lock().unwrap().push((image.to_owned(), fit));
                Ok(())
            }

            fn info(&self) -> BackendInfo {
                BackendInfo {
                    name: self.name.to_owned(),
                    detail: String::new(),
                }
            }
        }

        /// Re-detects to queued resolutions and builds the matching scripted setter.
        #[derive(Default)]
        struct FakeBackends {
            detections: Mutex<VecDeque<Resolution>>,
            setters: Mutex<Vec<(BackendKind, Arc<Scripted>)>>,
            redetects: Mutex<u32>,
            builds: Mutex<Vec<BackendKind>>,
            reconfigured: Mutex<Vec<DesktopConfig>>,
        }

        impl FakeBackends {
            fn detects(self: &Arc<Self>, res: Resolution) -> &Arc<Self> {
                self.detections.lock().unwrap().push_back(res);
                self
            }

            fn builds_into(self: &Arc<Self>, kind: BackendKind, setter: &Arc<Scripted>) {
                self.setters
                    .lock()
                    .unwrap()
                    .push((kind, Arc::clone(setter)));
            }

            fn redetects(&self) -> u32 {
                *self.redetects.lock().unwrap()
            }
        }

        #[async_trait::async_trait]
        impl Backends for FakeBackends {
            async fn redetect(&self) -> Resolution {
                *self.redetects.lock().unwrap() += 1;
                self.detections
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("a queued detection")
            }

            async fn build(&self, resolution: &Resolution) -> Result<Active, SetError> {
                self.builds.lock().unwrap().push(resolution.backend);
                let setters = self.setters.lock().unwrap();
                let (_, setter) = setters
                    .iter()
                    .find(|(kind, _)| *kind == resolution.backend)
                    .ok_or_else(|| SetError::Dbus("no session bus".to_owned()))?;
                Ok(Active::new(resolution.clone(), setter.clone()))
            }

            async fn resolve(&self, desktop: &DesktopConfig) -> Resolution {
                crate::linux::detect::resolve(&crate::linux::detect::Env::default(), desktop)
            }

            fn reconfigure(&self, desktop: &DesktopConfig) {
                self.reconfigured.lock().unwrap().push(desktop.clone());
            }
        }

        /// `[desktop]` pinning `backend` (no detection involved).
        fn pinned(backend: kakemono_config::schema::Backend) -> DesktopConfig {
            DesktopConfig {
                backend,
                managed_by: ManagedBy::User,
                ..DesktopConfig::default()
            }
        }

        fn img() -> &'static Path {
            Path::new("/tmp/a.jpg")
        }

        fn setter_with(
            kind: BackendKind,
            inner: &Arc<Scripted>,
            backends: Option<&Arc<FakeBackends>>,
        ) -> ResilientSetter {
            ResilientSetter::new(
                Active::new(resolution(kind), inner.clone()),
                backends.map(|b| Arc::clone(b) as Arc<dyn Backends>),
            )
        }

        #[tokio::test]
        async fn success_passes_through_and_resets_failures() {
            let feh = Scripted::new("feh", 1);
            let backends = Arc::new(FakeBackends::default());
            let setter = setter_with(BackendKind::Feh, &feh, Some(&backends));
            assert!(setter.set(img(), Fit::Fill).await.is_err());
            assert_eq!(setter.failures(), 1);
            setter.set(img(), Fit::Center).await.unwrap();
            assert_eq!(setter.failures(), 0);
            assert_eq!(feh.calls(), 1);
            assert_eq!(backends.redetects(), 0);
            assert_eq!(setter.info().name, "feh");
        }

        #[tokio::test]
        async fn second_consecutive_failure_switches_to_a_new_backend() {
            let feh = Scripted::new("feh", u32::MAX);
            let awww = Scripted::new("awww", 0);
            let backends = Arc::new(FakeBackends::default());
            backends
                .detects(resolution(BackendKind::Awww))
                .builds_into(BackendKind::Awww, &awww);
            let setter = setter_with(BackendKind::Feh, &feh, Some(&backends));

            let first = setter.set(img(), Fit::Fill).await.unwrap_err();
            assert_eq!(first.to_string(), "feh failed");
            assert_eq!(backends.redetects(), 0, "one failure is not enough");

            // The second failure re-detects and the same set succeeds on the new backend.
            setter.set(img(), Fit::Tile).await.unwrap();
            assert_eq!(backends.redetects(), 1);
            assert_eq!(*awww.calls.lock().unwrap(), [(img().to_owned(), Fit::Tile)]);
            assert_eq!(setter.resolution().backend, BackendKind::Awww);
            assert_eq!(setter.info().name, "awww");
            assert_eq!(setter.failures(), 0);

            // Later sets go straight to the new backend.
            setter.set(img(), Fit::Fill).await.unwrap();
            assert_eq!(awww.calls(), 2);
        }

        #[tokio::test]
        async fn same_backend_keeps_it_and_returns_the_original_error() {
            let feh = Scripted::new("feh", u32::MAX);
            let backends = Arc::new(FakeBackends::default());
            backends.detects(resolution(BackendKind::Feh));
            let setter = setter_with(BackendKind::Feh, &feh, Some(&backends));
            setter.set(img(), Fit::Fill).await.unwrap_err();
            let err = setter.set(img(), Fit::Fill).await.unwrap_err();
            assert_eq!(err.to_string(), "feh failed");
            assert_eq!(backends.redetects(), 1);
            assert!(
                backends.builds.lock().unwrap().is_empty(),
                "nothing rebuilt"
            );
            assert_eq!(setter.resolution().backend, BackendKind::Feh);
            // The counter restarted: the next single failure does not re-detect again.
            setter.set(img(), Fit::Fill).await.unwrap_err();
            assert_eq!(backends.redetects(), 1);
        }

        #[tokio::test]
        async fn failing_candidate_keeps_the_old_backend() {
            let feh = Scripted::new("feh", 2);
            let awww = Scripted::new("awww", 1);
            let backends = Arc::new(FakeBackends::default());
            backends
                .detects(resolution(BackendKind::Awww))
                .builds_into(BackendKind::Awww, &awww);
            let setter = setter_with(BackendKind::Feh, &feh, Some(&backends));
            setter.set(img(), Fit::Fill).await.unwrap_err();
            let err = setter.set(img(), Fit::Fill).await.unwrap_err();
            assert_eq!(
                err.to_string(),
                "feh failed",
                "the original error is reported"
            );
            assert_eq!(setter.resolution().backend, BackendKind::Feh);
            assert_eq!(awww.calls(), 0);
            // The old backend recovers.
            setter.set(img(), Fit::Fill).await.unwrap();
            assert_eq!(feh.calls(), 1);
        }

        #[tokio::test]
        async fn unbuildable_candidate_keeps_the_old_backend() {
            let feh = Scripted::new("feh", u32::MAX);
            let backends = Arc::new(FakeBackends::default());
            backends.detects(resolution(BackendKind::Kde));
            let setter = setter_with(BackendKind::Feh, &feh, Some(&backends));
            setter.set(img(), Fit::Fill).await.unwrap_err();
            let err = setter.set(img(), Fit::Fill).await.unwrap_err();
            assert_eq!(err.to_string(), "feh failed");
            assert_eq!(*backends.builds.lock().unwrap(), [BackendKind::Kde]);
            assert_eq!(setter.resolution().backend, BackendKind::Feh);
        }

        #[tokio::test]
        async fn unconfigured_heals_once_a_backend_appears() {
            let awww = Scripted::new("awww", 0);
            let backends = Arc::new(FakeBackends::default());
            backends
                .detects(resolution(BackendKind::Awww))
                .builds_into(BackendKind::Awww, &awww);
            let setter = ResilientSetter::new(
                Active::unconfigured(resolution(BackendKind::Unconfigured), "no backend yet"),
                Some(backends.clone() as Arc<dyn Backends>),
            );
            let err = setter.set(img(), Fit::Fill).await.unwrap_err();
            assert!(matches!(err, SetError::Unconfigured { .. }));
            setter.set(img(), Fit::Fill).await.unwrap();
            assert_eq!(setter.info().name, "awww");
        }

        #[tokio::test]
        async fn without_backends_it_never_redetects() {
            let feh = Scripted::new("feh", 5);
            let setter = setter_with(BackendKind::Feh, &feh, None);
            for _ in 0..5 {
                setter.set(img(), Fit::Fill).await.unwrap_err();
            }
            assert_eq!(setter.failures(), 5);
            setter.set(img(), Fit::Fill).await.unwrap();
            assert_eq!(setter.failures(), 0);
        }

        #[tokio::test]
        async fn rebuild_switches_to_the_new_backend() {
            use kakemono_config::schema::Backend;
            let feh = Scripted::new("feh", 0);
            let awww = Scripted::new("awww", 0);
            let backends = Arc::new(FakeBackends::default());
            backends.builds_into(BackendKind::Awww, &awww);
            let setter = ResilientSetter::for_desktop(
                Active::new(resolution(BackendKind::Feh), feh.clone()),
                backends.clone(),
                &DesktopConfig::default(),
            );
            assert!(setter.redetects(), "managed_by = detect by default");

            let new = setter
                .rebuild(&pinned(Backend::Awww))
                .await
                .unwrap()
                .expect("a new setter");
            assert_eq!(new.info().name, "awww");
            new.set(img(), Fit::Fill).await.unwrap();
            assert_eq!((feh.calls(), awww.calls()), (0, 1));
            assert_eq!(*backends.builds.lock().unwrap(), [BackendKind::Awww]);
            assert_eq!(
                *backends.reconfigured.lock().unwrap(),
                [pinned(Backend::Awww)]
            );
            assert_eq!(
                setter.info().name,
                "feh",
                "the old setter itself is unchanged"
            );
        }

        #[tokio::test]
        async fn rebuild_follows_managed_by() {
            use kakemono_config::schema::Backend;
            let feh = Scripted::new("feh", u32::MAX);
            let backends = Arc::new(FakeBackends::default());
            backends.builds_into(BackendKind::Feh, &feh);
            let setter = ResilientSetter::for_desktop(
                Active::new(resolution(BackendKind::Feh), feh.clone()),
                backends.clone(),
                &pinned(Backend::Feh),
            );
            assert!(!setter.redetects());
            let detect = DesktopConfig {
                managed_by: ManagedBy::Detect,
                ..pinned(Backend::Feh)
            };
            let new = setter.rebuild(&detect).await.unwrap().unwrap();
            new.set(img(), Fit::Fill).await.unwrap_err();
            backends.detects(resolution(BackendKind::Feh));
            new.set(img(), Fit::Fill).await.unwrap_err();
            assert_eq!(backends.redetects(), 1, "the rebuilt setter re-detects");
        }

        #[tokio::test]
        async fn rebuild_that_cannot_resolve_or_build_keeps_the_old_setter() {
            use kakemono_config::schema::Backend;
            let feh = Scripted::new("feh", 0);
            let backends = Arc::new(FakeBackends::default());
            let setter = ResilientSetter::for_desktop(
                Active::new(resolution(BackendKind::Feh), feh.clone()),
                backends.clone(),
                &DesktopConfig::default(),
            );

            // `backend = "command"` without `set_command` does not resolve to a usable backend.
            let Err(err) = setter.rebuild(&pinned(Backend::Command)).await else {
                panic!("rebuilt without a set_command");
            };
            assert!(matches!(err, SetError::Unconfigured { .. }), "{err:?}");
            assert!(err.to_string().starts_with("command backend: "), "{err}");
            assert!(backends.builds.lock().unwrap().is_empty(), "nothing built");

            // KDE resolves but cannot be built (FakeBackends has no session bus).
            let Err(err) = setter.rebuild(&pinned(Backend::Kde)).await else {
                panic!("built KDE without a session bus");
            };
            assert_eq!(err.to_string(), "D-Bus: no session bus", "{err:?}");

            assert!(backends.reconfigured.lock().unwrap().is_empty());
            setter.set(img(), Fit::Fill).await.unwrap();
            assert_eq!(feh.calls(), 1, "the old setter still works");
        }

        #[tokio::test]
        async fn without_backends_rebuild_keeps_the_setter() {
            let feh = Scripted::new("feh", 0);
            let setter = setter_with(BackendKind::Feh, &feh, None);
            let rebuilt = setter.rebuild(&DesktopConfig::default()).await.unwrap();
            assert!(rebuilt.is_none());
        }

        #[test]
        fn same_backend_compares_kind_command_and_persistence() {
            let a = resolution(BackendKind::Command);
            let mut b = a.clone();
            b.reasons.push("different reasons are fine".to_owned());
            b.origin = Origin::Config;
            assert!(same_backend(&a, &b));
            b.set_command = Some(vec!["x".to_owned()]);
            assert!(!same_backend(&a, &b));
            let mut c = a.clone();
            c.persistent = true;
            assert!(!same_backend(&a, &c));
            assert!(!same_backend(&a, &resolution(BackendKind::Feh)));
        }
    }
}

/// [`open_platform`] and the real [`Backends`]. Linux-only.
#[cfg(target_os = "linux")]
mod live {
    use std::fmt;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use kakemono_config::schema::{Backend, DesktopConfig};

    use super::backends::command::{CommandSetter, ThemeSource, render_error};
    use super::backends::gsettings::GsettingsSetter;
    use super::backends::kde::KdeSetter;
    use super::backends::persistent::{
        NativeProcesses, PID_FILE, PersistentSetter, ProcessControl, Supervisor, WATCH_INTERVAL,
    };
    use super::backends::xfce::XfceSetter;
    use super::detect::{self, BackendKind, Resolution};
    use super::resilient::{Active, Backends, ResilientSetter};
    use super::theme::LinuxSignals;
    use crate::api::{Platform, SetError, UNCONFIGURED_HINT, WallpaperSetter};
    use crate::exec::{CommandRunner, TokioRunner};

    /// Builds setters for resolutions and re-detects. One per daemon: it owns the session-bus
    /// connection (KDE, XFCE) and the one persistent-setter [`Supervisor`] (there is a single
    /// `setter.pid`).
    pub struct LiveBackends {
        /// `[desktop]` with `backend = "auto"` and no saved session, for re-detection. Replaced
        /// when a reload rebuilds the setter.
        redetect_cfg: Mutex<DesktopConfig>,
        /// The environment to resolve against instead of probing this session (tests).
        fixed_env: Option<detect::Env>,
        runner: Arc<dyn CommandRunner>,
        processes: Arc<dyn ProcessControl>,
        theme: ThemeSource,
        runtime_dir: Option<PathBuf>,
        session_bus: tokio::sync::OnceCell<zbus::Connection>,
        supervisor: Mutex<Option<(Arc<Supervisor>, tokio::task::JoinHandle<()>)>>,
    }

    impl fmt::Debug for LiveBackends {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("LiveBackends")
                .field("runtime_dir", &self.runtime_dir)
                .finish_non_exhaustive()
        }
    }

    impl LiveBackends {
        /// The real thing: commands through [`TokioRunner`], processes through
        /// [`NativeProcesses`]. `{theme}` comes from `theme`. `setter.pid` lives in `runtime_dir`
        /// and nowhere else; `None` disables persistent setters.
        pub fn new(cfg: &DesktopConfig, theme: ThemeSource, runtime_dir: Option<PathBuf>) -> Self {
            Self::with_parts(
                cfg,
                theme,
                Arc::new(TokioRunner),
                Arc::new(NativeProcesses::new()),
                runtime_dir,
            )
        }

        /// [`new`](Self::new) with an explicit command runner and process control (fakes in
        /// tests, so no real process is ever signalled).
        pub fn with_parts(
            cfg: &DesktopConfig,
            theme: ThemeSource,
            runner: Arc<dyn CommandRunner>,
            processes: Arc<dyn ProcessControl>,
            runtime_dir: Option<PathBuf>,
        ) -> Self {
            Self {
                redetect_cfg: Mutex::new(redetect_cfg(cfg)),
                fixed_env: None,
                runner,
                processes,
                theme,
                runtime_dir,
                session_bus: tokio::sync::OnceCell::new(),
                supervisor: Mutex::new(None),
            }
        }

        /// Resolve against `env` instead of probing the session (tests).
        #[must_use]
        pub fn with_env(mut self, env: detect::Env) -> Self {
            self.fixed_env = Some(env);
            self
        }

        /// The session environment: probed (with D-Bus), or the fixed one.
        async fn env(&self) -> detect::Env {
            match &self.fixed_env {
                Some(env) => env.clone(),
                None => detect::env::capture_with_dbus().await,
            }
        }

        async fn session_bus(&self) -> Result<zbus::Connection, SetError> {
            self.session_bus
                .get_or_try_init(zbus::Connection::session)
                .await
                .cloned()
                .map_err(|err| SetError::Dbus(format!("session bus: {err}")))
        }

        /// The supervisor, created (adopting `setter.pid`) and watched on first use.
        pub fn supervisor(&self) -> Result<Arc<Supervisor>, SetError> {
            let mut slot = self.supervisor.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((supervisor, _)) = slot.as_ref() {
                return Ok(Arc::clone(supervisor));
            }
            let dir = self
                .runtime_dir
                .as_ref()
                .ok_or_else(|| SetError::Unconfigured {
                    hint: "persistent setters need a runtime directory ($XDG_RUNTIME_DIR or the \
                       config directory)"
                        .to_owned(),
                })?;
            let supervisor = Arc::new(Supervisor::new(Arc::clone(&self.processes), dir));
            let watch = supervisor.watch(WATCH_INTERVAL);
            *slot = Some((Arc::clone(&supervisor), watch));
            Ok(supervisor)
        }

        /// At startup: if `active` is a usable backend that is not persistent, stop the
        /// persistent setter an earlier run left behind (it would cover the new wallpaper).
        /// Only `setter.pid` in this instance's runtime directory is read.
        pub async fn stop_stale_setter_if_replaced(&self, active: &Active) {
            if active.resolution.is_usable() && active.supervisor.is_none() {
                self.stop_stale_setter().await;
            }
        }

        /// Stop a persistent setter left by an earlier run (e.g. swaybg before switching to
        /// awww), if `setter.pid` in the runtime directory names one that is still ours.
        pub async fn stop_stale_setter(&self) {
            let has_record = self
                .runtime_dir
                .as_ref()
                .is_some_and(|dir| dir.join(PID_FILE).exists());
            if !has_record {
                return;
            }
            match self.supervisor() {
                Ok(supervisor) => {
                    if let Some(record) = supervisor.current().await {
                        tracing::info!(
                            pid = record.pid,
                            argv0 = record.argv.first().map_or("", String::as_str),
                            "stopping the previous persistent setter"
                        );
                    }
                    supervisor.stop().await;
                }
                Err(err) => tracing::debug!(%err, "cannot check for a stale setter"),
            }
        }

        /// [`build`](Backends::build), or an unconfigured setter that reports why it failed.
        pub async fn build_or_unconfigured(&self, resolution: Resolution) -> Active {
            match self.build(&resolution).await {
                Ok(active) => active,
                Err(err) => {
                    tracing::warn!(backend = %resolution.backend, %err, "cannot use the wallpaper backend");
                    Active::unconfigured(resolution, err.to_string())
                }
            }
        }
    }

    #[async_trait::async_trait]
    impl Backends for LiveBackends {
        async fn redetect(&self) -> Resolution {
            let env = self.env().await;
            let cfg = self
                .redetect_cfg
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let resolution = detect::resolve(&env, &cfg);
            tracing::info!(
                backend = %resolution.backend,
                reasons = ?resolution.reasons,
                "re-detected wallpaper backend"
            );
            resolution
        }

        async fn build(&self, resolution: &Resolution) -> Result<Active, SetError> {
            let kind = resolution.backend;
            if !resolution.is_usable() {
                let hint = resolution.hint.as_deref().unwrap_or(UNCONFIGURED_HINT);
                return Ok(Active::unconfigured(resolution.clone(), hint));
            }
            let setter: Arc<dyn WallpaperSetter> = match kind {
                BackendKind::Gnome | BackendKind::Cinnamon | BackendKind::Mate => Arc::new(
                    GsettingsSetter::new(kind, Arc::clone(&self.runner))
                        .ok_or_else(|| SetError::Os(format!("{kind} has no gsettings schema")))?,
                ),
                BackendKind::Kde => Arc::new(KdeSetter::new(self.session_bus().await?)),
                BackendKind::Xfce => Arc::new(XfceSetter::new(self.session_bus().await?)),
                _ if resolution.persistent => {
                    let supervisor = self.supervisor()?;
                    let setter =
                        PersistentSetter::for_resolution(resolution, Arc::clone(&supervisor))
                            .map_err(render_error)?
                            .ok_or_else(|| no_command(resolution))?
                            .with_theme(Arc::clone(&self.theme));
                    return Ok(Active {
                        resolution: resolution.clone(),
                        setter: Arc::new(setter),
                        supervisor: Some(supervisor),
                    });
                }
                _ => Arc::new(
                    CommandSetter::for_resolution(resolution, Arc::clone(&self.runner))
                        .map_err(render_error)?
                        .ok_or_else(|| no_command(resolution))?
                        .with_theme(Arc::clone(&self.theme)),
                ),
            };
            Ok(Active::new(resolution.clone(), setter))
        }

        async fn resolve(&self, desktop: &DesktopConfig) -> Resolution {
            let resolution = detect::resolve(&self.env().await, desktop);
            tracing::info!(
                backend = %resolution.backend,
                origin = ?resolution.origin,
                session = %resolution.session_key,
                reasons = ?resolution.reasons,
                "wallpaper backend for the reloaded [desktop]"
            );
            resolution
        }

        fn reconfigure(&self, desktop: &DesktopConfig) {
            *self.redetect_cfg.lock().unwrap_or_else(|e| e.into_inner()) = redetect_cfg(desktop);
        }
    }

    /// `cfg` with `backend = "auto"` and no saved session: what re-detection resolves.
    fn redetect_cfg(cfg: &DesktopConfig) -> DesktopConfig {
        DesktopConfig {
            backend: Backend::Auto,
            detected_session: None,
            detected_at: None,
            ..cfg.clone()
        }
    }

    fn no_command(resolution: &Resolution) -> SetError {
        SetError::Unconfigured {
            hint: resolution
                .hint
                .clone()
                .unwrap_or_else(|| format!("{} has no command to run", resolution.backend)),
        }
    }

    /// Build the Linux platform from `[desktop]`. Polled inside the tokio runtime; `Send`.
    ///
    /// Signals: [`LinuxSignals::start`]. Setter: detection (`detect::resolve` on a D-Bus-probed
    /// env), the matching backend, wrapped in a [`ResilientSetter`] that re-detects only when
    /// `managed_by = "detect"`. Never fails: a backend that cannot start becomes an
    /// unconfigured setter that reports why.
    pub async fn open_platform(cfg: &DesktopConfig) -> Platform {
        let runtime_dir = match kakemono_config::paths::runtime_dir() {
            Ok(dir) => Some(dir),
            Err(err) => {
                tracing::warn!(%err, "no runtime directory; persistent setters are disabled");
                None
            }
        };
        open_platform_in(cfg, runtime_dir).await
    }

    /// [`open_platform`] with `setter.pid` in `runtime_dir` instead of the user's runtime
    /// directory. Only that directory's `setter.pid` is ever read, adopted or stopped; `None`
    /// disables persistent setters.
    pub async fn open_platform_in(cfg: &DesktopConfig, runtime_dir: Option<PathBuf>) -> Platform {
        let signals = Arc::new(LinuxSignals::start(cfg).await);
        let backends = Arc::new(LiveBackends::new(cfg, signals.theme_source(), runtime_dir));
        Platform {
            setter: Arc::new(open_setter(backends, cfg).await),
            signals,
        }
    }

    /// The setter for `cfg` at startup: resolve, build (or an unconfigured setter that says
    /// why), stop a persistent setter an earlier run left if this backend replaces it, and wrap
    /// it in a [`ResilientSetter`] that can rebuild itself on reload.
    pub async fn open_setter(backends: Arc<LiveBackends>, cfg: &DesktopConfig) -> ResilientSetter {
        let env = backends.env().await;
        let resolution = detect::resolve(&env, cfg);
        tracing::info!(
            backend = %resolution.backend,
            origin = ?resolution.origin,
            session = %resolution.session_key,
            reasons = ?resolution.reasons,
            "wallpaper backend"
        );
        let active = backends.build_or_unconfigured(resolution).await;
        backends.stop_stale_setter_if_replaced(&active).await;
        ResilientSetter::for_desktop(active, backends, cfg)
    }

    #[cfg(test)]
    mod tests {
        use std::collections::HashMap;
        use std::io;
        use std::path::Path;
        use std::sync::Mutex as StdMutex;
        use std::time::Duration;

        use super::*;
        use crate::api::{CommandStatus, Fit, Theme};
        use crate::exec::{Invocation, Output, RunError};
        use crate::linux::backends::persistent::{Record, write_record};
        use crate::linux::detect::Origin;

        /// Fake process table: `spawn` adds a live entry (PIDs from 1000), `comm` answers from
        /// `live`, `terminate` records and removes. No real process is ever started or signalled.
        #[derive(Debug, Default)]
        struct FakeProcesses {
            live: StdMutex<HashMap<u32, String>>,
            spawned: StdMutex<Vec<Vec<String>>>,
            terminated: StdMutex<Vec<u32>>,
        }

        impl FakeProcesses {
            fn with_live(procs: &[(u32, &str)]) -> Arc<Self> {
                let fake = Self::default();
                fake.live
                    .lock()
                    .unwrap()
                    .extend(procs.iter().map(|(pid, comm)| (*pid, (*comm).to_owned())));
                Arc::new(fake)
            }

            fn terminated(&self) -> Vec<u32> {
                self.terminated.lock().unwrap().clone()
            }

            fn live_pids(&self) -> Vec<u32> {
                let mut pids: Vec<u32> = self.live.lock().unwrap().keys().copied().collect();
                pids.sort_unstable();
                pids
            }
        }

        #[async_trait::async_trait]
        impl ProcessControl for FakeProcesses {
            fn spawn(&self, argv: &[String]) -> io::Result<u32> {
                let mut spawned = self.spawned.lock().unwrap();
                spawned.push(argv.to_vec());
                let pid = 999 + u32::try_from(spawned.len()).unwrap();
                let comm = crate::linux::backends::persistent::expected_comm(&argv[0]);
                self.live.lock().unwrap().insert(pid, comm);
                Ok(pid)
            }

            fn comm(&self, pid: u32) -> Option<String> {
                self.live.lock().unwrap().get(&pid).cloned()
            }

            async fn terminate(&self, pid: u32, _grace: Duration) {
                self.terminated.lock().unwrap().push(pid);
                self.live.lock().unwrap().remove(&pid);
            }
        }

        #[derive(Default)]
        struct Recorder {
            runs: StdMutex<Vec<Vec<String>>>,
        }

        #[async_trait::async_trait]
        impl CommandRunner for Recorder {
            async fn run(&self, invocation: &Invocation) -> Result<Output, RunError> {
                self.runs.lock().unwrap().push(invocation.argv.clone());
                Ok(Output {
                    status: CommandStatus::Exited(0),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }

            fn spawn_detached(&self, _argv: &[String]) -> Result<u32, RunError> {
                unreachable!("not persistent")
            }
        }

        fn resolution(backend: BackendKind, set_command: Option<&[&str]>) -> Resolution {
            Resolution {
                backend,
                set_command: set_command.map(|c| c.iter().map(|s| (*s).to_owned()).collect()),
                persistent: backend.is_persistent(),
                origin: Origin::Config,
                session_key: "test:x11".to_owned(),
                hint: None,
                reasons: Vec::new(),
            }
        }

        fn backends(runner: &Arc<Recorder>, theme: Theme) -> LiveBackends {
            LiveBackends::with_parts(
                &DesktopConfig::default(),
                Arc::new(move || theme),
                Arc::clone(runner) as Arc<dyn CommandRunner>,
                FakeProcesses::with_live(&[]),
                None,
            )
        }

        /// A `LiveBackends` whose `setter.pid` lives in `runtime_dir`, with fake processes.
        fn backends_in(processes: &Arc<FakeProcesses>, runtime_dir: Option<&Path>) -> LiveBackends {
            LiveBackends::with_parts(
                &DesktopConfig::default(),
                Arc::new(|| Theme::Light),
                Arc::new(Recorder::default()),
                Arc::clone(processes) as Arc<dyn ProcessControl>,
                runtime_dir.map(Path::to_path_buf),
            )
        }

        /// Write a `setter.pid` in `dir` naming a swaybg with `pid`.
        fn record_swaybg(dir: &Path, pid: u32) {
            let record = Record {
                pid,
                argv: vec!["swaybg".to_owned(), "-i".to_owned(), "/w/a.jpg".to_owned()],
            };
            write_record(&dir.join(PID_FILE), &record).unwrap();
        }

        #[tokio::test]
        async fn startup_stops_a_stale_setter_only_in_the_configured_dir() {
            let configured = tempfile::tempdir().unwrap();
            let elsewhere = tempfile::tempdir().unwrap();
            record_swaybg(configured.path(), 4242);
            record_swaybg(elsewhere.path(), 5151);
            let processes = FakeProcesses::with_live(&[(4242, "swaybg"), (5151, "swaybg")]);
            let live = backends_in(&processes, Some(configured.path()));

            let gnome = live
                .build(&resolution(BackendKind::Gnome, None))
                .await
                .unwrap();
            live.stop_stale_setter_if_replaced(&gnome).await;

            assert_eq!(
                processes.terminated(),
                [4242],
                "only the configured dir's setter"
            );
            assert!(!configured.path().join(PID_FILE).exists());
            assert!(
                elsewhere.path().join(PID_FILE).exists(),
                "a setter.pid outside the configured dir is never read or removed"
            );
        }

        #[tokio::test]
        async fn startup_without_a_runtime_dir_stops_nothing() {
            let elsewhere = tempfile::tempdir().unwrap();
            record_swaybg(elsewhere.path(), 5151);
            let processes = FakeProcesses::with_live(&[(5151, "swaybg")]);
            let live = backends_in(&processes, None);

            let gnome = live
                .build(&resolution(BackendKind::Gnome, None))
                .await
                .unwrap();
            live.stop_stale_setter_if_replaced(&gnome).await;

            assert!(processes.terminated().is_empty());
            assert!(elsewhere.path().join(PID_FILE).exists());
        }

        #[tokio::test]
        async fn startup_keeps_the_setter_for_persistent_or_unusable_backends() {
            let dir = tempfile::tempdir().unwrap();
            record_swaybg(dir.path(), 4242);
            let processes = FakeProcesses::with_live(&[(4242, "swaybg")]);
            let live = backends_in(&processes, Some(dir.path()));

            // Persistent: the supervisor adopts the running swaybg instead of stopping it.
            let swaybg = live
                .build(&resolution(BackendKind::Swaybg, None))
                .await
                .unwrap();
            assert!(swaybg.supervisor.is_some());
            live.stop_stale_setter_if_replaced(&swaybg).await;

            // Unusable: nothing would replace the old wallpaper, so leave it.
            let none = live
                .build_or_unconfigured(resolution(BackendKind::Unconfigured, None))
                .await;
            live.stop_stale_setter_if_replaced(&none).await;

            assert!(processes.terminated().is_empty());
            assert!(dir.path().join(PID_FILE).exists());
        }

        /// The real `open_platform` path against this session's buses and desktop, but with a
        /// private runtime dir so it can never stop a real persistent setter. Never calls `set`.
        /// Opt-in like `macos_set_roundtrip`:
        /// `KAKEMONO_REAL_DESKTOP_TEST=1 cargo test -p kakemono-platform --lib -- --ignored`
        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "probes the real Linux desktop session; also needs KAKEMONO_REAL_DESKTOP_TEST=1"]
        async fn real_open_platform_in_a_private_runtime_dir() {
            if std::env::var_os("KAKEMONO_REAL_DESKTOP_TEST").is_none_or(|v| v != "1") {
                eprintln!("skipped: set KAKEMONO_REAL_DESKTOP_TEST=1");
                return;
            }
            let dir = tempfile::tempdir().unwrap();
            let platform =
                open_platform_in(&DesktopConfig::default(), Some(dir.path().to_path_buf())).await;
            let info = platform.setter.info();
            eprintln!(
                "backend {} ({}), theme {:?}",
                info.name,
                info.detail,
                platform.signals.theme()
            );
        }

        #[tokio::test]
        async fn builds_gsettings_and_command_setters() {
            let runner = Arc::new(Recorder::default());
            let live = backends(&runner, Theme::Dark);

            let gnome = live
                .build(&resolution(BackendKind::Gnome, None))
                .await
                .unwrap();
            assert_eq!(gnome.setter.info().name, "gnome");
            assert!(gnome.supervisor.is_none());

            let cmd = resolution(BackendKind::Command, Some(&["setbg", "{path}", "{theme}"]));
            let active = live.build(&cmd).await.unwrap();
            active
                .setter
                .set(std::path::Path::new("/w/a.jpg"), Fit::Fill)
                .await
                .unwrap();
            assert_eq!(
                runner.runs.lock().unwrap().last().unwrap(),
                &["setbg", "/w/a.jpg", "dark"],
                "{{theme}} comes from the theme source"
            );
        }

        #[tokio::test]
        async fn unusable_and_unbuildable_resolutions_report_why() {
            let runner = Arc::new(Recorder::default());
            let live = backends(&runner, Theme::Light);

            let mut none = resolution(BackendKind::Unconfigured, None);
            none.hint = Some("run doctor".to_owned());
            let active = live.build(&none).await.unwrap();
            let err = active
                .setter
                .set(std::path::Path::new("/a.jpg"), Fit::Fill)
                .await;
            assert_eq!(err.unwrap_err().to_string(), "run doctor");

            // Persistent without a runtime directory cannot start.
            let swaybg = resolution(BackendKind::Swaybg, None);
            let err = live.build(&swaybg).await.unwrap_err();
            assert!(err.to_string().contains("runtime directory"), "{err}");
            let active = live.build_or_unconfigured(swaybg).await;
            let err = active
                .setter
                .set(std::path::Path::new("/a.jpg"), Fit::Fill)
                .await;
            assert!(err.unwrap_err().to_string().contains("runtime directory"));
        }

        /// `[desktop]` pinning `backend` (and `set_command`), so resolving needs no session.
        fn pinned(backend: Backend, set_command: Option<&[&str]>) -> DesktopConfig {
            DesktopConfig {
                backend,
                managed_by: kakemono_config::schema::ManagedBy::User,
                set_command: set_command.map(|argv| {
                    kakemono_config::schema::CommandSpec::new(
                        argv.iter().map(|s| (*s).to_owned()).collect(),
                    )
                    .unwrap()
                }),
                ..DesktopConfig::default()
            }
        }

        /// Task 37b: a reload hands the one `Supervisor` over. Persistent → persistent: the old
        /// process runs until the new one replaced it. Persistent → anything else: it is stopped
        /// and `setter.pid` removed. A `[desktop]` that does not resolve changes nothing.
        #[tokio::test]
        async fn reload_swaps_the_persistent_setter_without_leaking_it() {
            let dir = tempfile::tempdir().unwrap();
            let pid_file = dir.path().join(PID_FILE);
            let processes = FakeProcesses::with_live(&[]);
            let runner = Arc::new(Recorder::default());
            let live = Arc::new(
                LiveBackends::with_parts(
                    &DesktopConfig::default(),
                    Arc::new(|| Theme::Light),
                    Arc::clone(&runner) as Arc<dyn CommandRunner>,
                    Arc::clone(&processes) as Arc<dyn ProcessControl>,
                    Some(dir.path().to_path_buf()),
                )
                .with_env(detect::Env::default()),
            );
            let record = || crate::linux::backends::persistent::read_record(&pid_file);
            let img = |name: &str| PathBuf::from(format!("/w/{name}"));

            let swaybg: Arc<dyn WallpaperSetter> =
                Arc::new(open_setter(Arc::clone(&live), &pinned(Backend::Swaybg, None)).await);
            swaybg.set(&img("a.jpg"), Fit::Fill).await.unwrap();
            let first = record().expect("setter.pid after the first set");
            assert_eq!(first.argv[0], "swaybg");

            // swaybg → wbg: nothing stops until the new setter replaced the old process.
            let wbg = swaybg
                .rebuild(&pinned(Backend::Wbg, None))
                .await
                .unwrap()
                .expect("rebuilt");
            assert_eq!(wbg.info().name, "wbg");
            assert!(processes.terminated().is_empty());
            assert_eq!(
                record(),
                Some(first.clone()),
                "still names the running swaybg"
            );
            wbg.set(&img("b.jpg"), Fit::Fill).await.unwrap();
            let second = record().unwrap();
            assert_eq!(second.argv[0], "wbg");
            assert_eq!(processes.terminated(), [first.pid]);
            assert_eq!(processes.live_pids(), [second.pid]);

            // A `[desktop]` that does not resolve: the error, and nothing stopped.
            let Err(err) = wbg.rebuild(&pinned(Backend::Command, None)).await else {
                panic!("rebuilt without a set_command");
            };
            assert!(err.to_string().starts_with("command backend: "), "{err}");
            assert_eq!(processes.live_pids(), [second.pid]);
            assert_eq!(record(), Some(second.clone()));

            // wbg → a command: the persistent setter is stopped and setter.pid removed.
            let command = wbg
                .rebuild(&pinned(Backend::Command, Some(&["setbg", "{path}"])))
                .await
                .unwrap()
                .expect("rebuilt");
            assert_eq!(processes.terminated(), [first.pid, second.pid]);
            assert!(processes.live_pids().is_empty(), "nothing leaked");
            assert!(!pid_file.exists(), "no stale setter.pid");
            command.set(&img("c.jpg"), Fit::Fill).await.unwrap();
            assert_eq!(
                runner.runs.lock().unwrap().last().unwrap(),
                &["setbg", "/w/c.jpg"]
            );
            assert_eq!(command.info().detail, "setbg {path}");

            // Back to swaybg: the same supervisor starts it on the next set.
            let again = command
                .rebuild(&pinned(Backend::Swaybg, None))
                .await
                .unwrap()
                .expect("rebuilt");
            assert!(processes.live_pids().is_empty());
            again.set(&img("d.jpg"), Fit::Fill).await.unwrap();
            let third = record().unwrap();
            assert_eq!(third.argv[0], "swaybg");
            assert_eq!(processes.live_pids(), [third.pid]);
            assert!(again.info().detail.ends_with(" (kept running)"));
            assert_eq!(processes.spawned.lock().unwrap().len(), 3);
        }

        #[test]
        fn redetection_ignores_the_saved_backend() {
            let cfg = DesktopConfig {
                backend: Backend::Feh,
                detected_session: Some("i3:x11".to_owned()),
                ..DesktopConfig::default()
            };
            let live = LiveBackends::with_parts(
                &cfg,
                Arc::new(|| Theme::Light),
                Arc::new(TokioRunner),
                FakeProcesses::with_live(&[]),
                None,
            );
            let redetect = live.redetect_cfg.lock().unwrap().clone();
            assert_eq!(redetect.backend, Backend::Auto);
            assert_eq!(redetect.detected_session, None);
        }
    }
}

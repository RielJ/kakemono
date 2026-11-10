//! [`ResilientSetter`]: re-detect in memory after repeated `SetError`s (plan §E, "Failure,
//! fallback and re-detection").

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use kakemono_config::schema::{DesktopConfig, ManagedBy};

use crate::api::{
    BackendInfo, Fit, SetError, UNCONFIGURED_HINT, UnconfiguredSetter, WallpaperSetter,
};
use crate::deadline::{SET_TIMEOUT, set_within};
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
        if let Err(retry_err) = set_within(candidate.setter.as_ref(), image, fit, SET_TIMEOUT).await
        {
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
        let set = set_within(active.setter.as_ref(), image, fit, SET_TIMEOUT);
        let err = match set.await {
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
                return Err(SetError::os(format!("{} failed", self.name)));
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
                .ok_or_else(|| SetError::dbus("no session bus", "no session bus"))?;
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

    /// A backend whose `set` never returns (a frozen plasmashell, xfconfd or Explorer).
    struct Hanging;

    #[async_trait::async_trait]
    impl WallpaperSetter for Hanging {
        async fn set(&self, _image: &Path, _fit: Fit) -> Result<(), SetError> {
            std::future::pending().await
        }

        fn info(&self) -> BackendInfo {
            BackendInfo {
                name: "kde".to_owned(),
                detail: String::new(),
            }
        }
    }

    /// H3: a backend that never answers fails after `SET_TIMEOUT` instead of holding the
    /// engine's op lock forever, and the timeout counts towards re-detection.
    #[tokio::test(start_paused = true)]
    async fn a_hanging_set_times_out_and_counts_as_a_failure() {
        let awww = Scripted::new("awww", 0);
        let backends = Arc::new(FakeBackends::default());
        backends
            .detects(resolution(BackendKind::Awww))
            .builds_into(BackendKind::Awww, &awww);
        let setter = ResilientSetter::new(
            Active::new(resolution(BackendKind::Kde), Arc::new(Hanging)),
            Some(backends.clone() as Arc<dyn Backends>),
        );

        let started = tokio::time::Instant::now();
        let err = setter.set(img(), Fit::Fill).await.unwrap_err();
        assert_eq!(started.elapsed(), SET_TIMEOUT);
        assert!(
            matches!(&err, SetError::TimedOut { backend, after } if backend == "kde" && *after == SET_TIMEOUT),
            "{err:?}"
        );
        assert_eq!(err.to_string(), "kde: timed out after 35s");
        assert_eq!(setter.failures(), 1);

        // The second timeout re-detects and the retry succeeds on the new backend.
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

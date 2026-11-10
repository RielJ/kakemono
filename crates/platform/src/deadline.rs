//! A time limit on every `WallpaperSetter::set` (audit H3).
//!
//! KDE's `evaluateScript`, XFCE's D-Bus calls and the Windows desktop-thread reply have no
//! timeout of their own. The engine holds its op lock during `set`, so one frozen plasmashell,
//! xfconfd or Explorer would block every later next, prev and reload. Linux applies
//! [`set_within`] in `ResilientSetter`; Windows wraps its setter in [`Deadline`].

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use kakemono_config::schema::DesktopConfig;

use crate::api::{BackendInfo, Fit, SetError, WallpaperSetter};

/// How long one `set` may take. Longer than [`crate::exec::DEFAULT_TIMEOUT`], so a command
/// backend reports its own (more precise) timeout first.
pub const SET_TIMEOUT: Duration = Duration::from_secs(35);

/// `setter.set(image, fit)`, or `SetError::TimedOut` after `after`. The pending call is dropped
/// on timeout; whatever it started (a D-Bus call, a queued request) is abandoned, not undone.
///
/// # Errors
///
/// `setter`'s own error, or `SetError::TimedOut` naming `setter.info().name`.
pub async fn set_within(
    setter: &dyn WallpaperSetter,
    image: &Path,
    fit: Fit,
    after: Duration,
) -> Result<(), SetError> {
    match tokio::time::timeout(after, setter.set(image, fit)).await {
        Ok(result) => result,
        Err(_) => {
            let backend = setter.info().name;
            tracing::warn!(%backend, ?after, "wallpaper backend did not answer; giving up on this set");
            Err(SetError::TimedOut { backend, after })
        }
    }
}

/// `inner` with every `set` limited to `after` ([`set_within`]). `info` and `rebuild` pass
/// through.
#[derive(Debug)]
pub struct Deadline<S> {
    inner: S,
    after: Duration,
}

impl<S> Deadline<S> {
    pub fn new(inner: S, after: Duration) -> Self {
        Self { inner, after }
    }
}

#[async_trait::async_trait]
impl<S: WallpaperSetter> WallpaperSetter for Deadline<S> {
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
        set_within(&self.inner, image, fit, self.after).await
    }

    fn info(&self) -> BackendInfo {
        self.inner.info()
    }

    async fn rebuild(
        &self,
        desktop: &DesktopConfig,
    ) -> Result<Option<Arc<dyn WallpaperSetter>>, SetError> {
        self.inner.rebuild(desktop).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Never answers, like a frozen Explorer.
    struct Hanging;

    #[async_trait::async_trait]
    impl WallpaperSetter for Hanging {
        async fn set(&self, _image: &Path, _fit: Fit) -> Result<(), SetError> {
            std::future::pending().await
        }

        fn info(&self) -> BackendInfo {
            BackendInfo {
                name: "windows".to_owned(),
                detail: String::new(),
            }
        }
    }

    struct Quick;

    #[async_trait::async_trait]
    impl WallpaperSetter for Quick {
        async fn set(&self, _image: &Path, _fit: Fit) -> Result<(), SetError> {
            Ok(())
        }

        fn info(&self) -> BackendInfo {
            BackendInfo {
                name: "quick".to_owned(),
                detail: String::new(),
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_hanging_set_fails_after_the_deadline() {
        let setter = Deadline::new(Hanging, SET_TIMEOUT);
        let started = tokio::time::Instant::now();
        let err = setter
            .set(Path::new("/a.png"), Fit::Fill)
            .await
            .unwrap_err();
        assert_eq!(started.elapsed(), SET_TIMEOUT);
        assert!(
            matches!(&err, SetError::TimedOut { backend, after } if backend == "windows" && *after == SET_TIMEOUT),
            "{err:?}"
        );
        assert_eq!(err.to_string(), "windows: timed out after 35s");
        assert_eq!(setter.info().name, "windows");
    }

    #[tokio::test(start_paused = true)]
    async fn a_prompt_set_passes_through() {
        let setter = Deadline::new(Quick, SET_TIMEOUT);
        setter.set(Path::new("/a.png"), Fit::Fill).await.unwrap();
        assert!(
            setter
                .rebuild(&DesktopConfig::default())
                .await
                .unwrap()
                .is_none()
        );
    }
}

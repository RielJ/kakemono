//! Engine events, broadcast to `subscribe` clients and the UI (plan §D).
//!
//! `Engine` owns the sender. `next`, `prev`, `set_file` and `trash` send `Changed`;
//! `pause`/`resume` send `Paused` when the flag flips; the rotation loop (Task 21) sends
//! `ThemeChanged`. The IPC server (Task 45) forwards each one to `subscribe` clients as a
//! [`kakemono_protocol::types::Event`] through the `From` impl below.

use kakemono_platform::api::Theme;
use kakemono_protocol::types::{Event, WallpaperInfo};

/// What the engine broadcasts after a change. Receivers that lag lose the oldest events
/// (`tokio::sync::broadcast` semantics).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineEvent {
    /// The wallpaper changed (new image, history step, `set`, or `trash` followed by `next`).
    Changed(WallpaperInfo),
    /// Auto-rotation was paused (`true`) or resumed (`false`).
    Paused(bool),
    /// The system theme changed (sent by the rotation loop, Task 21).
    ThemeChanged(Theme),
}

/// The wire form: `{"event":"changed","data":…}`, `{"event":"paused","data":true}`,
/// `{"event":"theme_changed","data":"dark"}`.
impl From<EngineEvent> for Event {
    fn from(event: EngineEvent) -> Self {
        match event {
            EngineEvent::Changed(info) => Event::Changed(info),
            EngineEvent::Paused(paused) => Event::Paused(paused),
            EngineEvent::ThemeChanged(theme) => Event::ThemeChanged(theme.as_str().to_owned()),
        }
    }
}

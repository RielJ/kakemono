//! Engine events, broadcast to `subscribe` clients and the UI (plan §D).
//!
//! TODO(task 22): conversion to `kakemono_protocol::types::Event` and the tests for broadcasting
//! from every op. Task 19 defines the type because `Engine` owns the sender: `next`, `prev`,
//! `set_file` and `trash` send `Changed`, `pause`/`resume` send `Paused` when the flag flips.

use kakemono_platform::api::Theme;
use kakemono_protocol::types::WallpaperInfo;

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

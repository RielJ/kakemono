//! KDE Plasma: set the wallpaper with a Plasma desktop script over D-Bus.
//!
//! [`script_for`] builds JavaScript that switches every desktop containment (`desktops()`, one
//! per screen and activity) to the `org.kde.image` plugin and writes `Image` and `FillMode`.
//! [`KdeSetter`] sends it to `org.kde.plasmashell` `/PlasmaShell`
//! `org.kde.PlasmaShell.evaluateScript`.
//!
//! The image URI goes into the script only as a JSON string literal (`serde_json::to_string`),
//! so no path can break out of it and run code.

use std::fmt;
use std::path::Path;

use crate::api::{BackendInfo, Fit, SetError, Theme, WallpaperSetter};
use crate::exec::Vars;
use crate::linux::backends::command::render_error;

/// Plasma's bus name.
pub const DESTINATION: &str = "org.kde.plasmashell";
/// The scripting object.
pub const PATH: &str = "/PlasmaShell";
/// The scripting interface.
pub const INTERFACE: &str = "org.kde.PlasmaShell";
/// The method that runs a desktop script and returns what it `print`s.
pub const METHOD: &str = "evaluateScript";
/// The wallpaper plugin every desktop is switched to.
pub const PLUGIN: &str = "org.kde.image";

/// `org.kde.image`'s `FillMode` (Qt `Image.FillMode`) for `fit`: Stretch 0, PreserveAspectFit 1,
/// PreserveAspectCrop 2, Tile 3, Pad 6.
pub fn fill_mode(fit: Fit) -> u8 {
    match fit {
        Fit::Fill => 2,
        Fit::Fit => 1,
        Fit::Center => 6,
        Fit::Stretch => 0,
        Fit::Tile => 3,
    }
}

/// A JavaScript string literal for `s`. JSON strings are JS strings, except that U+2028 and
/// U+2029 end a line in pre-ES2019 engines, so those are escaped too.
fn js_string(s: &str) -> String {
    serde_json::to_string(s)
        .unwrap_or_else(|_| "\"\"".to_owned())
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// The Plasma script that sets `uri` with `fit` on every desktop. `uri` is embedded only as a
/// string literal.
pub fn script_for(uri: &str, fit: Fit) -> String {
    format!(
        "var uri = {uri};\n\
         var fillMode = {fill};\n\
         var all = desktops();\n\
         for (var i = 0; i < all.length; i++) {{\n\
         \x20   var d = all[i];\n\
         \x20   d.wallpaperPlugin = {plugin};\n\
         \x20   d.currentConfigGroup = [\"Wallpaper\", {plugin}, \"General\"];\n\
         \x20   d.writeConfig(\"Image\", uri);\n\
         \x20   d.writeConfig(\"FillMode\", fillMode);\n\
         }}\n",
        uri = js_string(uri),
        fill = fill_mode(fit),
        plugin = js_string(PLUGIN),
    )
}

/// A [`WallpaperSetter`] that runs [`script_for`] in Plasma over a D-Bus connection.
#[derive(Clone)]
pub struct KdeSetter {
    conn: zbus::Connection,
}

impl fmt::Debug for KdeSetter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KdeSetter").finish_non_exhaustive()
    }
}

impl KdeSetter {
    /// A setter on `conn` (the session bus in production; a private connection in tests).
    pub fn new(conn: zbus::Connection) -> Self {
        Self { conn }
    }

    /// The script `set(image, fit)` would send (for `doctor`'s dry run).
    pub fn render(&self, image: &Path, fit: Fit) -> Result<String, SetError> {
        let vars = Vars::new(image, fit, Theme::Light).map_err(render_error)?;
        Ok(script_for(vars.uri(), fit))
    }
}

#[async_trait::async_trait]
impl WallpaperSetter for KdeSetter {
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
        let script = self.render(image, fit)?;
        tracing::debug!(%script, "running Plasma desktop script");
        let reply = self
            .conn
            .call_method(Some(DESTINATION), PATH, Some(INTERFACE), METHOD, &(script,))
            .await
            .map_err(|err| {
                let message = format!(
                    "{INTERFACE}.{METHOD}: {err} (Plasma must be running, with widgets unlocked)"
                );
                SetError::dbus(message, err)
            })?;
        if let Ok(printed) = reply.body().deserialize::<String>()
            && !printed.trim().is_empty()
        {
            tracing::debug!(output = printed.trim(), "Plasma desktop script output");
        }
        Ok(())
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: "kde".to_owned(),
            detail: format!("D-Bus {DESTINATION} {PATH} {INTERFACE}.{METHOD} ({PLUGIN})"),
        }
    }
}

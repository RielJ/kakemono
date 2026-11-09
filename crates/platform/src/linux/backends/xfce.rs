//! XFCE: set the wallpaper through xfconf over D-Bus (plan §E, Task 33).
//!
//! `org.xfce.Xfconf.GetAllProperties("xfce4-desktop", "/backdrop")` lists every backdrop
//! property. Each `…/last-image` key is one monitor × workspace. For each, [`steps`] plans a
//! `SetProperty` of the image path and of the sibling `…/image-style` from [`Fit`].
//!
//! xfdesktop ignores a write of the value a key already holds, so if a key already shows this
//! path it is first set to `""` (as Variety does) to force a redraw.
//!
//! There are no `last-image` keys until a wallpaper has been set once in Desktop Settings; `set`
//! then fails with that hint instead of guessing monitor names.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::Path;

use zbus::{OwnedVariant, Variant};

use crate::api::{BackendInfo, Fit, SetError, Theme, WallpaperSetter};
use crate::exec::Vars;
use crate::linux::backends::command::render_error;

/// xfconf's bus name.
pub const DESTINATION: &str = "org.xfce.Xfconf";
/// xfconf's object path.
pub const PATH: &str = "/org/xfce/Xfconf";
/// xfconf's interface.
pub const INTERFACE: &str = "org.xfce.Xfconf";
/// xfdesktop's channel.
pub const CHANNEL: &str = "xfce4-desktop";
/// Where the backdrop properties live.
pub const BACKDROP: &str = "/backdrop";

const LAST_IMAGE: &str = "/last-image";
const IMAGE_STYLE: &str = "/image-style";

/// xfdesktop's `image-style` for `fit`: centered 1, tiled 2, stretched 3, scaled 4, zoomed 5.
pub fn image_style(fit: Fit) -> i32 {
    match fit {
        Fit::Center => 1,
        Fit::Tile => 2,
        Fit::Stretch => 3,
        Fit::Fit => 4,
        Fit::Fill => 5,
    }
}

/// The `…/last-image` keys under `/backdrop/`, sorted.
pub fn last_image_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = keys
        .into_iter()
        .filter(|k| k.starts_with("/backdrop/") && k.ends_with(LAST_IMAGE))
        .map(str::to_owned)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The `…/image-style` key next to a `…/last-image` key.
pub fn style_key(last_image_key: &str) -> String {
    let base = last_image_key
        .strip_suffix(LAST_IMAGE)
        .unwrap_or(last_image_key);
    format!("{base}{IMAGE_STYLE}")
}

/// One `SetProperty` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A string property (`last-image`).
    Text { key: String, value: String },
    /// An int property (`image-style`).
    Int { key: String, value: i32 },
}

/// The `SetProperty` calls that put `path` on every monitor and workspace with `fit`.
/// `current` maps each backdrop key to its string value (`None` for non-strings).
pub fn steps(current: &BTreeMap<String, Option<String>>, path: &str, fit: Fit) -> Vec<Step> {
    let mut out = Vec::new();
    for key in last_image_keys(current.keys().map(String::as_str)) {
        if current.get(&key).and_then(Option::as_deref) == Some(path) {
            out.push(Step::Text {
                key: key.clone(),
                value: String::new(),
            });
        }
        out.push(Step::Int {
            key: style_key(&key),
            value: image_style(fit),
        });
        out.push(Step::Text {
            key,
            value: path.to_owned(),
        });
    }
    out
}

/// The hint when xfconf has no `last-image` keys yet.
pub const NO_KEYS_HINT: &str = "xfce4-desktop has no /backdrop/…/last-image property yet; set a \
                                wallpaper once in Settings → Desktop, then retry";

/// A [`WallpaperSetter`] that writes xfconf over a D-Bus connection.
#[derive(Clone)]
pub struct XfceSetter {
    conn: zbus::Connection,
}

impl fmt::Debug for XfceSetter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("XfceSetter").finish_non_exhaustive()
    }
}

fn dbus_error(method: &str, err: &zbus::Error) -> SetError {
    SetError::Dbus(format!("{INTERFACE}.{method}: {err}"))
}

impl XfceSetter {
    /// A setter on `conn` (the session bus in production; a private connection in tests).
    pub fn new(conn: zbus::Connection) -> Self {
        Self { conn }
    }

    /// Every `/backdrop` property with its string value (`None` for other types).
    pub async fn backdrop(&self) -> Result<BTreeMap<String, Option<String>>, SetError> {
        let reply = self
            .conn
            .call_method(
                Some(DESTINATION),
                PATH,
                Some(INTERFACE),
                "GetAllProperties",
                &(CHANNEL, BACKDROP),
            )
            .await
            .map_err(|err| dbus_error("GetAllProperties", &err))?;
        let props: HashMap<String, OwnedVariant> = reply
            .body()
            .deserialize()
            .map_err(|err| dbus_error("GetAllProperties", &err))?;
        Ok(props
            .into_iter()
            .map(|(key, value)| {
                let text = match &*value {
                    Variant::Str(s) => Some(s.as_str().to_owned()),
                    _ => None,
                };
                (key, text)
            })
            .collect())
    }

    async fn set_property(&self, key: &str, value: Variant<'_>) -> Result<(), SetError> {
        self.conn
            .call_method(
                Some(DESTINATION),
                PATH,
                Some(INTERFACE),
                "SetProperty",
                &(CHANNEL, key, value),
            )
            .await
            .map_err(|err| dbus_error("SetProperty", &err))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl WallpaperSetter for XfceSetter {
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
        let vars = Vars::new(image, fit, Theme::Light).map_err(render_error)?;
        let plan = steps(&self.backdrop().await?, vars.path(), fit);
        if plan.is_empty() {
            return Err(SetError::Unconfigured {
                hint: NO_KEYS_HINT.to_owned(),
            });
        }
        tracing::debug!(?plan, "writing xfconf backdrop properties");
        for step in plan {
            match step {
                Step::Text { key, value } => self.set_property(&key, Variant::from(value)).await?,
                Step::Int { key, value } => self.set_property(&key, Variant::from(value)).await?,
            }
        }
        Ok(())
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: "xfce".to_owned(),
            detail: format!("D-Bus {DESTINATION} {CHANNEL} {BACKDROP}/…/last-image"),
        }
    }
}

//! `linux::backends::xfce` (plan Task 33). Key selection is pure and tested on sample property
//! maps. The D-Bus tests serve a fake `org.xfce.Xfconf` with `#[zbus::interface]` on a
//! peer-to-peer Unix socket pair: no session bus, no `dbus-daemon`.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, HashMap};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use kakemono_platform::linux::backends::xfce::{
    NO_KEYS_HINT, Step, XfceSetter, image_style, last_image_keys, steps, style_key,
};
use kakemono_platform::{Fit, SetError, WallpaperSetter};
use zbus::connection::Builder;
use zbus::{OwnedVariant, Variant};

/// Two monitors, two workspaces on the first, plus unrelated keys. Shaped like XFCE 4.18.
fn sample() -> BTreeMap<String, Option<String>> {
    [
        (
            "/backdrop/screen0/monitoreDP-1/workspace0/last-image",
            Some("/old/a.jpg"),
        ),
        (
            "/backdrop/screen0/monitoreDP-1/workspace0/image-style",
            None,
        ),
        (
            "/backdrop/screen0/monitoreDP-1/workspace0/color-style",
            None,
        ),
        (
            "/backdrop/screen0/monitoreDP-1/workspace1/last-image",
            Some("/w/new.jpg"),
        ),
        (
            "/backdrop/screen0/monitorHDMI-A-1/workspace0/last-image",
            Some(""),
        ),
        (
            "/backdrop/screen0/monitorHDMI-A-1/workspace0/last-image-x",
            Some("decoy"),
        ),
        ("/backdrop/single-workspace-mode", None),
        ("/desktop-icons/last-image", Some("not a backdrop")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.map(str::to_owned)))
    .collect()
}

#[test]
fn xfce_selects_every_last_image_key() {
    let keys = last_image_keys(sample().keys().map(String::as_str));
    assert_eq!(
        keys,
        [
            "/backdrop/screen0/monitorHDMI-A-1/workspace0/last-image",
            "/backdrop/screen0/monitoreDP-1/workspace0/last-image",
            "/backdrop/screen0/monitoreDP-1/workspace1/last-image",
        ]
    );
    assert_eq!(
        style_key("/backdrop/screen0/monitorX/workspace3/last-image"),
        "/backdrop/screen0/monitorX/workspace3/image-style"
    );
    assert!(last_image_keys([]).is_empty());
    assert!(last_image_keys(["/backdrop/screen0/monitor0/workspace0/image-style"]).is_empty());
}

#[test]
fn xfce_image_styles() {
    let styles: Vec<i32> = [Fit::Fill, Fit::Fit, Fit::Center, Fit::Stretch, Fit::Tile]
        .into_iter()
        .map(image_style)
        .collect();
    assert_eq!(styles, [5, 4, 1, 3, 2]);
}

#[test]
fn xfce_steps_set_style_then_image_and_reset_unchanged_paths() {
    let plan = steps(&sample(), "/w/new.jpg", Fit::Fit);
    let text = |key: &str, value: &str| Step::Text {
        key: key.to_owned(),
        value: value.to_owned(),
    };
    let int = |key: &str| Step::Int {
        key: key.to_owned(),
        value: 4,
    };
    assert_eq!(
        plan,
        [
            int("/backdrop/screen0/monitorHDMI-A-1/workspace0/image-style"),
            text(
                "/backdrop/screen0/monitorHDMI-A-1/workspace0/last-image",
                "/w/new.jpg"
            ),
            int("/backdrop/screen0/monitoreDP-1/workspace0/image-style"),
            text(
                "/backdrop/screen0/monitoreDP-1/workspace0/last-image",
                "/w/new.jpg"
            ),
            // Already showing this path: blank it first so xfdesktop redraws.
            text("/backdrop/screen0/monitoreDP-1/workspace1/last-image", ""),
            int("/backdrop/screen0/monitoreDP-1/workspace1/image-style"),
            text(
                "/backdrop/screen0/monitoreDP-1/workspace1/last-image",
                "/w/new.jpg"
            ),
        ]
    );
    assert!(steps(&BTreeMap::new(), "/w/new.jpg", Fit::Fill).is_empty());
}

#[derive(Clone, Default)]
struct FakeXfconf {
    props: Arc<Mutex<BTreeMap<String, OwnedVariant>>>,
    calls: Arc<Mutex<Vec<(String, String, OwnedVariant)>>>,
}

impl FakeXfconf {
    fn with(props: &[(&str, Variant<'static>)]) -> Self {
        let fake = Self::default();
        for (key, value) in props {
            fake.props
                .lock()
                .unwrap()
                .insert((*key).to_owned(), value.try_to_owned().unwrap());
        }
        fake
    }

    fn sets(&self) -> Vec<(String, String)> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, key, value)| (key.clone(), format!("{}", **value)))
            .collect()
    }
}

#[zbus::interface(name = "org.xfce.Xfconf")]
impl FakeXfconf {
    fn get_all_properties(
        &self,
        channel: String,
        base: String,
    ) -> zbus::fdo::Result<HashMap<String, OwnedVariant>> {
        if channel != "xfce4-desktop" {
            return Err(zbus::fdo::Error::Failed(format!("no channel {channel}")));
        }
        let props = self.props.lock().unwrap();
        let out: HashMap<String, OwnedVariant> = props
            .iter()
            .filter(|(k, _)| k.starts_with(&base))
            .map(|(k, v)| (k.clone(), v.try_clone().unwrap()))
            .collect();
        if out.is_empty() {
            return Err(zbus::fdo::Error::Failed(format!(
                "Property \"{base}\" does not exist on channel \"{channel}\""
            )));
        }
        Ok(out)
    }

    fn set_property(&self, channel: String, property: String, value: OwnedVariant) {
        self.props
            .lock()
            .unwrap()
            .insert(property.clone(), value.try_clone().unwrap());
        self.calls.lock().unwrap().push((channel, property, value));
    }
}

/// A client connection whose peer serves `xfconf` at `/org/xfce/Xfconf`. Keep the server alive.
async fn p2p(xfconf: FakeXfconf) -> (zbus::Connection, zbus::Connection) {
    let (a, b) = UnixStream::pair().unwrap();
    let server = Builder::unix_stream(a)
        .server(zbus::Guid::generate())
        .p2p()
        .serve_at("/org/xfce/Xfconf", xfconf)
        .build();
    let client = Builder::unix_stream(b).p2p().build();
    let (server, client) = tokio::join!(server, client);
    (client.unwrap(), server.unwrap())
}

#[tokio::test]
async fn xfce_setter_writes_every_monitor_with_typed_values() {
    let fake = FakeXfconf::with(&[
        (
            "/backdrop/screen0/monitorDP-1/workspace0/last-image",
            Variant::from("/old.jpg"),
        ),
        (
            "/backdrop/screen0/monitorDP-1/workspace0/image-style",
            Variant::from(5i32),
        ),
        (
            "/backdrop/screen0/monitorDP-2/workspace0/last-image",
            Variant::from("/w/x y.png"),
        ),
        ("/backdrop/single-workspace-mode", Variant::from(true)),
    ]);
    let (client, _server) = p2p(fake.clone()).await;
    let setter = XfceSetter::new(client);

    let current = setter.backdrop().await.unwrap();
    assert_eq!(current["/backdrop/single-workspace-mode"], None);
    assert_eq!(
        current["/backdrop/screen0/monitorDP-1/workspace0/last-image"].as_deref(),
        Some("/old.jpg")
    );

    setter
        .set(Path::new("/w/x y.png"), Fit::Tile)
        .await
        .unwrap();
    assert_eq!(
        fake.sets(),
        [
            (
                "/backdrop/screen0/monitorDP-1/workspace0/image-style".into(),
                "2".into()
            ),
            (
                "/backdrop/screen0/monitorDP-1/workspace0/last-image".into(),
                "\"/w/x y.png\"".into()
            ),
            (
                "/backdrop/screen0/monitorDP-2/workspace0/last-image".into(),
                "\"\"".into()
            ),
            (
                "/backdrop/screen0/monitorDP-2/workspace0/image-style".into(),
                "2".into()
            ),
            (
                "/backdrop/screen0/monitorDP-2/workspace0/last-image".into(),
                "\"/w/x y.png\"".into()
            ),
        ]
    );
    let calls = fake.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .all(|(channel, _, _)| channel == "xfce4-desktop")
    );
    // image-style must be an int32 and last-image a string, or xfdesktop ignores them.
    assert!(matches!(&*calls[0].2, Variant::I32(2)));
    assert!(matches!(&*calls[1].2, Variant::Str(_)));
    assert_eq!(XfceSetter::new(_server.clone()).info().name, "xfce");
}

#[tokio::test]
async fn xfce_setter_without_keys_gives_a_hint() {
    let fake = FakeXfconf::with(&[("/backdrop/single-workspace-mode", Variant::from(true))]);
    let (client, _server) = p2p(fake.clone()).await;
    match XfceSetter::new(client)
        .set(Path::new("/w/x.png"), Fit::Fill)
        .await
    {
        Err(SetError::Unconfigured { hint }) => assert_eq!(hint, NO_KEYS_HINT),
        other => panic!("{other:?}"),
    }
    assert!(fake.sets().is_empty());
}

#[tokio::test]
async fn xfce_setter_maps_dbus_errors() {
    let (client, _server) = p2p(FakeXfconf::default()).await;
    match XfceSetter::new(client)
        .set(Path::new("/w/x.png"), Fit::Fill)
        .await
    {
        Err(SetError::Dbus(msg)) => {
            assert!(msg.contains("GetAllProperties"), "{msg}");
            assert!(msg.contains("does not exist"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
}

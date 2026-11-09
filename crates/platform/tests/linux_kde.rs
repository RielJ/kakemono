//! `linux::backends::kde` (plan Task 32). The D-Bus tests serve a fake `org.kde.PlasmaShell` with
//! `#[zbus::interface]` on a peer-to-peer Unix socket pair: no session bus, no `dbus-daemon`.
#![cfg(target_os = "linux")]

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use kakemono_platform::exec::Vars;
use kakemono_platform::linux::backends::kde::{KdeSetter, fill_mode, script_for};
use kakemono_platform::{Fit, SetError, Theme, WallpaperSetter};
use zbus::connection::Builder;

/// The JSON string literal that follows `var uri = ` in `script`, and the script with it cut out.
fn split_uri_literal(script: &str) -> (String, String) {
    let prefix = "var uri = ";
    let start = script.find(prefix).expect("uri declaration") + prefix.len();
    let mut stream = serde_json::Deserializer::from_str(&script[start..]).into_iter::<String>();
    let value = stream.next().expect("a literal").expect("a JSON string");
    let end = start + stream.byte_offset();
    (value, format!("{}{}", &script[..start], &script[end..]))
}

#[test]
fn kde_script_snapshot() {
    insta::assert_snapshot!(
        "kde_script",
        script_for("file:///home/u/Pictures/a%20b.jpg", Fit::Fill)
    );
}

#[test]
fn kde_fill_modes() {
    let modes: Vec<u8> = [Fit::Fill, Fit::Fit, Fit::Center, Fit::Stretch, Fit::Tile]
        .into_iter()
        .map(fill_mode)
        .collect();
    assert_eq!(modes, [2, 1, 6, 0, 3]);
    assert!(script_for("file:///x", Fit::Tile).contains("var fillMode = 3;"));
}

/// A hostile string stays one string literal: the script outside the literal is identical to the
/// script for a harmless URI.
#[test]
fn kde_injection_stays_inside_string_literal() {
    let benign = script_for("file:///w/x.jpg", Fit::Fill);
    let (_, benign_rest) = split_uri_literal(&benign);
    for hostile in [
        r#"file:///w/"); evil(); (".jpg"#,
        "file:///w/\\\"); evil(); //",
        "file:///w/a\nevil();\n'",
        "file:///w/\u{2028}evil();\u{2029}",
        "file:///w/</script><script>evil()</script>",
    ] {
        let script = script_for(hostile, Fit::Fill);
        let (literal, rest) = split_uri_literal(&script);
        assert_eq!(literal, hostile);
        assert_eq!(
            rest, benign_rest,
            "code outside the literal changed for {hostile:?}"
        );
        assert!(!script.contains('\u{2028}') && !script.contains('\u{2029}'));
    }

    // Through the setter's path → URI step the quote is percent-encoded as well.
    let vars = Vars::new(
        Path::new(r#"/w/"); evil(); (".jpg"#),
        Fit::Fill,
        Theme::Light,
    )
    .unwrap();
    let (literal, rest) = split_uri_literal(&script_for(vars.uri(), Fit::Fill));
    assert_eq!(literal, "file:///w/%22);%20evil();%20(%22.jpg");
    assert_eq!(rest, benign_rest);
}

#[derive(Clone, Default)]
struct FakePlasma {
    scripts: Arc<Mutex<Vec<String>>>,
    fail: bool,
}

#[zbus::interface(name = "org.kde.PlasmaShell")]
impl FakePlasma {
    #[zbus(name = "evaluateScript")]
    fn evaluate_script(&self, script: String) -> zbus::fdo::Result<String> {
        self.scripts.lock().unwrap().push(script);
        if self.fail {
            Err(zbus::fdo::Error::Failed("Widgets are locked".to_owned()))
        } else {
            Ok(String::new())
        }
    }
}

/// A client connection whose peer serves `plasma` at `/PlasmaShell`. Keep the server alive.
async fn p2p(plasma: FakePlasma) -> (zbus::Connection, zbus::Connection) {
    let (a, b) = UnixStream::pair().unwrap();
    let server = Builder::unix_stream(a)
        .server(zbus::Guid::generate())
        .p2p()
        .serve_at("/PlasmaShell", plasma)
        .build();
    let client = Builder::unix_stream(b).p2p().build();
    let (server, client) = tokio::join!(server, client);
    (client.unwrap(), server.unwrap())
}

#[tokio::test]
async fn kde_setter_calls_evaluate_script() {
    let plasma = FakePlasma::default();
    let (client, _server) = p2p(plasma.clone()).await;
    let setter = KdeSetter::new(client);
    let image = Path::new("/home/u/it's \"x\".png");
    setter.set(image, Fit::Center).await.unwrap();

    let scripts = plasma.scripts.lock().unwrap().clone();
    assert_eq!(scripts.len(), 1);
    assert_eq!(scripts[0], setter.render(image, Fit::Center).unwrap());
    let (literal, _) = split_uri_literal(&scripts[0]);
    assert_eq!(literal, "file:///home/u/it's%20%22x%22.png");
    assert!(scripts[0].contains("var fillMode = 6;"));
    assert_eq!(setter.info().name, "kde");
}

#[tokio::test]
async fn kde_setter_maps_dbus_errors() {
    let plasma = FakePlasma {
        fail: true,
        ..FakePlasma::default()
    };
    let (client, _server) = p2p(plasma).await;
    let err = KdeSetter::new(client)
        .set(Path::new("/w/x.png"), Fit::Fill)
        .await
        .unwrap_err();
    match &err {
        SetError::Dbus(msg) => {
            assert!(msg.contains("Widgets are locked"), "{msg}");
            assert!(msg.contains("evaluateScript"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn kde_setter_rejects_relative_paths_before_calling() {
    let plasma = FakePlasma::default();
    let (client, _server) = p2p(plasma.clone()).await;
    let err = KdeSetter::new(client)
        .set(Path::new("x.png"), Fit::Fill)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("absolute"), "{err}");
    assert!(plasma.scripts.lock().unwrap().is_empty());
}

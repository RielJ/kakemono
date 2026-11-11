//! Picker placement without a display: edge geometry, saved-position checks, platform
//! selection, and the Hyprland/sway placers against fixture `hyprctl`/`swaymsg` JSON with the
//! issued argv snapshotted.

use std::collections::HashMap;
use std::sync::Mutex;

use kakemono_config::schema::{PickerCustom, PickerOrientation, PickerPosition};
use kakemono_ui::picker_place::{
    FLOATING_HORIZONTAL, FLOATING_VERTICAL, Fallback, HyprPlacer, Monitor, PICKER_TITLE,
    PlaceError, Placer, PlacerKind, Rect, Restore, Runner, StripEnd, SwayPlacer, check_custom,
    edge_rect, fit_custom, fit_strip, floating_size, needs_x11_relaunch, placer_kind, resize_strip,
};

const PID: u32 = 4242;

/// Replays canned stdout per argv and records every call.
struct FakeRunner {
    replies: HashMap<String, String>,
    calls: Mutex<Vec<String>>,
}

impl FakeRunner {
    fn new(replies: &[(&str, &str)]) -> Self {
        Self {
            replies: replies
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> String {
        self.calls.lock().unwrap().join("\n")
    }
}

impl Runner for &FakeRunner {
    fn run(&self, argv: &[&str]) -> Result<String, PlaceError> {
        // Quote args with spaces so the snapshot shows argv boundaries.
        let shown: Vec<String> = argv
            .iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("'{a}'")
                } else {
                    (*a).to_owned()
                }
            })
            .collect();
        self.calls.lock().unwrap().push(shown.join(" "));
        let key = if argv.len() == 3 && argv[0] == "swaymsg" {
            "swaymsg-command".to_owned()
        } else if argv.get(1) == Some(&"dispatch") {
            "hyprctl-dispatch".to_owned()
        } else {
            argv.join(" ")
        };
        self.replies
            .get(&key)
            .cloned()
            .ok_or_else(|| PlaceError::Failed {
                cmd: key,
                message: "no fixture".into(),
            })
    }
}

fn custom(x: i32, y: i32, monitor: &str) -> PickerCustom {
    PickerCustom {
        x,
        y,
        monitor: monitor.into(),
        length: 0,
    }
}

// ── Fixtures (trimmed real output) ──────────────────────────────────────────

/// `hyprctl clients -j`: the picker (ours), a same-titled window from another pid, a
/// terminal. DP-1 is 2560x1440 at scale 1, eDP-1 is 2880x1800 at scale 2 left of it.
const HYPR_CLIENTS: &str = r#"[
  {"address":"0x55d1c0a0","mapped":true,"hidden":false,"at":[3060,1250],"size":[1000,160],
   "workspace":{"id":1,"name":"1"},"floating":true,"monitor":1,"class":"kakemono-ui",
   "title":"kakemono-picker","initialClass":"kakemono-ui","initialTitle":"kakemono-picker",
   "pid":4242,"xwayland":false,"pinned":false,"fullscreen":0},
  {"address":"0x55d1c0b0","mapped":true,"hidden":false,"at":[0,0],"size":[100,100],
   "workspace":{"id":2,"name":"2"},"floating":true,"monitor":0,"class":"other",
   "title":"kakemono-picker","pid":9999,"xwayland":false},
  {"address":"0x55d1c0c0","mapped":true,"hidden":false,"at":[1450,10],"size":[2540,1420],
   "workspace":{"id":1,"name":"1"},"floating":false,"monitor":1,"class":"foot",
   "title":"~","pid":1200,"xwayland":false}
]"#;

const HYPR_MONITORS: &str = r#"[
  {"id":0,"name":"eDP-1","description":"Built-in","make":"BOE","model":"0x0BCA",
   "width":2880,"height":1800,"refreshRate":60.0,"x":0,"y":0,"scale":2.0,"transform":0,
   "focused":false},
  {"id":1,"name":"DP-1","description":"Dell U2720Q","make":"Dell","model":"U2720Q",
   "width":2560,"height":1440,"refreshRate":59.95,"x":1440,"y":0,"scale":1.0,"transform":0,
   "focused":true}
]"#;

/// `swaymsg -r -t get_tree`: root → outputs → workspace → floating picker.
const SWAY_TREE: &str = r#"{
  "id":1,"type":"root","name":"root","rect":{"x":0,"y":0,"width":4000,"height":1440},
  "nodes":[
    {"id":2147483647,"type":"output","name":"__i3","rect":{"x":0,"y":0,"width":0,"height":0},
     "nodes":[],"floating_nodes":[]},
    {"id":3,"type":"output","name":"eDP-1","rect":{"x":0,"y":0,"width":1440,"height":900},
     "nodes":[{"id":4,"type":"workspace","name":"1","rect":{"x":0,"y":0,"width":1440,"height":900},
       "nodes":[],"floating_nodes":[]}],"floating_nodes":[]},
    {"id":5,"type":"output","name":"DP-1","rect":{"x":1440,"y":0,"width":2560,"height":1440},
     "nodes":[{"id":6,"type":"workspace","name":"2","rect":{"x":1440,"y":0,"width":2560,"height":1440},
       "nodes":[{"id":7,"type":"con","name":"~","app_id":"foot","pid":1200,
         "rect":{"x":1440,"y":0,"width":2560,"height":1440},"nodes":[],"floating_nodes":[]}],
       "floating_nodes":[{"id":8,"type":"floating_con","name":"kakemono-picker",
         "app_id":"kakemono-ui","pid":4242,"rect":{"x":2220,"y":1240,"width":1000,"height":160},
         "nodes":[],"floating_nodes":[]}]}],
     "floating_nodes":[]}
  ]
}"#;

const SWAY_OUTPUTS: &str = r#"[
  {"name":"eDP-1","active":true,"rect":{"x":0,"y":0,"width":1440,"height":900},"scale":2.0},
  {"name":"DP-1","active":true,"rect":{"x":1440,"y":0,"width":2560,"height":1440},"scale":1.0},
  {"name":"HDMI-A-1","active":false,"rect":{"x":0,"y":0,"width":0,"height":0}}
]"#;

fn hypr() -> FakeRunner {
    FakeRunner::new(&[
        ("hyprctl clients -j", HYPR_CLIENTS),
        ("hyprctl monitors -j", HYPR_MONITORS),
        ("hyprctl-dispatch", "ok"),
    ])
}

fn sway() -> FakeRunner {
    FakeRunner::new(&[
        ("swaymsg -r -t get_tree", SWAY_TREE),
        ("swaymsg -r -t get_outputs", SWAY_OUTPUTS),
        ("swaymsg-command", r#"[{"success":true}]"#),
    ])
}

// ── Hyprland ────────────────────────────────────────────────────────────────

#[test]
fn picker_place_hyprland_capture_reads_our_window_and_its_monitor() {
    let runner = hypr();
    let placer = HyprPlacer::new(&runner, PID);
    assert_eq!(placer.capture().unwrap(), Some(custom(3060, 1250, "DP-1")));
}

#[test]
fn picker_place_hyprland_restore_argv() {
    let runner = hypr();
    let placer = HyprPlacer::new(&runner, PID);
    let restored = placer.restore(&custom(1500, 1200, "DP-1")).unwrap();
    assert_eq!(
        restored,
        Restore::Placed {
            x: 1500,
            y: 1200,
            length: 0
        }
    );
    insta::assert_snapshot!(runner.calls(), @r"
    hyprctl clients -j
    hyprctl monitors -j
    hyprctl dispatch movewindowpixel 'exact 1500 1200,title:^(kakemono-picker)$'
    ");
}

#[test]
fn picker_place_hyprland_scaled_monitor_uses_layout_px() {
    // eDP-1 is 2880x1800 at scale 2 → 1440x900 layout px. At y = 800 a 160 px window hangs
    // 60 px below it, so it moves up to 740; in physical px (1800) it would have stayed.
    let runner = hypr();
    let restored = HyprPlacer::new(&runner, PID)
        .restore(&custom(400, 800, "eDP-1"))
        .unwrap();
    assert_eq!(
        restored,
        Restore::Placed {
            x: 400,
            y: 740,
            length: 0
        }
    );
    // y = 890 leaves 10 px of a 160 px window on screen: off screen.
    let runner = hypr();
    let restored = HyprPlacer::new(&runner, PID)
        .restore(&custom(400, 890, "eDP-1"))
        .unwrap();
    assert_eq!(restored, Restore::Fallback(Fallback::OffScreen));
}

#[test]
fn picker_place_hyprland_off_screen_falls_back_to_edge_without_moving() {
    let runner = hypr();
    let placer = HyprPlacer::new(&runner, PID);
    let restored = placer.restore(&custom(9000, 9000, "DP-1")).unwrap();
    assert_eq!(restored, Restore::Fallback(Fallback::OffScreen));
    assert!(!runner.calls().contains("dispatch"), "{}", runner.calls());
}

#[test]
fn picker_place_hyprland_missing_monitor_falls_back_to_edge() {
    let runner = hypr();
    let restored = HyprPlacer::new(&runner, PID)
        .restore(&custom(100, 100, "HDMI-A-1"))
        .unwrap();
    assert_eq!(restored, Restore::Fallback(Fallback::MonitorMissing));
}

#[test]
fn picker_place_hyprland_unmapped_picker_is_reported() {
    let runner = hypr();
    let placer = HyprPlacer::new(&runner, 1); // no window of pid 1
    assert_eq!(placer.capture().unwrap(), None);
    assert_eq!(
        placer.restore(&custom(1500, 1200, "DP-1")).unwrap(),
        Restore::Fallback(Fallback::NotMapped)
    );
}

#[test]
fn picker_place_hyprland_dispatch_error_is_an_error() {
    let runner = FakeRunner::new(&[
        ("hyprctl clients -j", HYPR_CLIENTS),
        ("hyprctl monitors -j", HYPR_MONITORS),
        ("hyprctl-dispatch", "No such window found"),
    ]);
    let err = HyprPlacer::new(&runner, PID)
        .restore(&custom(1500, 1200, "DP-1"))
        .unwrap_err();
    assert!(err.to_string().contains("No such window found"), "{err}");
}

// ── sway ────────────────────────────────────────────────────────────────────

#[test]
fn picker_place_sway_capture_reads_rect_and_output() {
    let runner = sway();
    let placer = SwayPlacer::new(&runner, PID);
    assert_eq!(placer.capture().unwrap(), Some(custom(2220, 1240, "DP-1")));
}

#[test]
fn picker_place_sway_restore_argv() {
    let runner = sway();
    let restored = SwayPlacer::new(&runner, PID)
        .restore(&custom(100, 700, "eDP-1"))
        .unwrap();
    assert_eq!(
        restored,
        Restore::Placed {
            x: 100,
            y: 700,
            length: 0
        }
    );
    insta::assert_snapshot!(runner.calls(), @r#"
    swaymsg -r -t get_tree
    swaymsg -r -t get_outputs
    swaymsg -r '[title="^kakemono-picker$" pid=4242] move absolute position 100 700'
    "#);
}

#[test]
fn picker_place_sway_off_screen_and_inactive_output_fall_back() {
    let runner = sway();
    let placer = SwayPlacer::new(&runner, PID);
    assert_eq!(
        placer.restore(&custom(-5000, 0, "DP-1")).unwrap(),
        Restore::Fallback(Fallback::OffScreen)
    );
    assert_eq!(
        placer.restore(&custom(0, 0, "HDMI-A-1")).unwrap(),
        Restore::Fallback(Fallback::MonitorMissing),
        "inactive outputs don't count"
    );
    assert!(!runner.calls().contains("move"), "{}", runner.calls());
}

#[test]
fn picker_place_sway_failed_command_is_an_error() {
    let runner = FakeRunner::new(&[
        ("swaymsg -r -t get_tree", SWAY_TREE),
        ("swaymsg -r -t get_outputs", SWAY_OUTPUTS),
        (
            "swaymsg-command",
            r#"[{"success":false,"parse_error":false,"error":"No matching node."}]"#,
        ),
    ]);
    let err = SwayPlacer::new(&runner, PID)
        .restore(&custom(100, 700, "eDP-1"))
        .unwrap_err();
    assert!(err.to_string().contains("No matching node."), "{err}");
}

// ── Geometry ────────────────────────────────────────────────────────────────

fn monitors() -> Vec<Monitor> {
    vec![
        Monitor {
            name: "Built-in Retina Display".into(),
            rect: Rect::new(0, 0, 3024, 1964),
        },
        Monitor {
            name: "DELL U2720Q".into(),
            rect: Rect::new(3024, -200, 3840, 2160),
        },
    ]
}

#[test]
fn picker_place_custom_on_a_screen_is_used() {
    let rect = check_custom(&custom(3100, 1500, "DELL U2720Q"), (2000, 320), &monitors());
    assert_eq!(rect, Ok(Rect::new(3100, 1500, 2000, 320)));
}

#[test]
fn picker_place_custom_hanging_off_a_screen_is_pulled_back_in() {
    let mons = monitors();
    // Past the built-in display's left edge: x snaps to 0, y is kept.
    assert_eq!(
        check_custom(
            &custom(-900, 100, "Built-in Retina Display"),
            (2000, 320),
            &mons
        ),
        Ok(Rect::new(0, 100, 2000, 320))
    );
    // Past the Dell's bottom-right corner (it ends at 6864, 1960).
    assert_eq!(
        check_custom(&custom(5500, 1800, "DELL U2720Q"), (2000, 320), &mons),
        Ok(Rect::new(4864, 1640, 2000, 320))
    );
    // A full-width strip dragged sideways goes back to the monitor's left edge.
    assert_eq!(
        check_custom(
            &custom(700, 1644, "Built-in Retina Display"),
            (3024, 320),
            &mons
        ),
        Ok(Rect::new(0, 1644, 3024, 320))
    );
}

#[test]
fn picker_place_custom_straddling_two_screens_moves_onto_the_one_it_covers_most() {
    // 1500 px on the built-in display, 500 px on the Dell.
    assert_eq!(
        check_custom(
            &custom(1524, 100, "Built-in Retina Display"),
            (2000, 320),
            &monitors()
        ),
        Ok(Rect::new(1024, 100, 2000, 320))
    );
}

#[test]
fn picker_place_custom_bigger_than_its_screen_starts_at_the_screen_edge() {
    assert_eq!(
        check_custom(
            &custom(-50, 40, "Built-in Retina Display"),
            (4000, 320),
            &monitors()
        ),
        Ok(Rect::new(0, 40, 4000, 320))
    );
}

#[test]
fn picker_place_off_screen_rect_falls_back_to_edge() {
    let mons = monitors();
    let size = (2000, 320);
    // Entirely below every monitor.
    assert_eq!(
        check_custom(&custom(0, 5000, "DELL U2720Q"), size, &mons),
        Err(Fallback::OffScreen)
    );
    // Only a 10 px sliver left on screen.
    assert_eq!(
        check_custom(&custom(-1990, 100, "Built-in Retina Display"), size, &mons),
        Err(Fallback::OffScreen)
    );
    // The monitor was unplugged.
    assert_eq!(
        check_custom(&custom(100, 100, "LG HDR 4K"), size, &mons),
        Err(Fallback::MonitorMissing)
    );
    // No monitors at all.
    assert_eq!(
        check_custom(&custom(100, 100, "LG HDR 4K"), size, &[]),
        Err(Fallback::MonitorMissing)
    );
}

// ── Strip fitting and resizing ─────────────────────────────────────────────

/// The built-in display's area, and 320 logical px at scale 2.
const AREA: Rect = Rect {
    x: 0,
    y: 0,
    w: 3024,
    h: 1964,
};
const MIN: u32 = 640;

#[test]
fn picker_fit_strip_dropped_three_quarters_across_ends_at_the_screen_edge() {
    assert_eq!(
        fit_strip(Rect::new(2268, 1600, 3024, 320), AREA, false, MIN),
        Rect::new(2268, 1600, 756, 320)
    );
}

#[test]
fn picker_fit_strip_keeps_the_minimum_length_by_sliding_back() {
    // Only 424 px would be left on screen: it slides left to keep 640.
    assert_eq!(
        fit_strip(Rect::new(2600, 1600, 3024, 320), AREA, false, MIN),
        Rect::new(2384, 1600, 640, 320)
    );
}

#[test]
fn picker_fit_strip_dropped_past_the_start_keeps_its_far_end() {
    assert_eq!(
        fit_strip(Rect::new(-1000, 100, 3024, 320), AREA, false, MIN),
        Rect::new(0, 100, 2024, 320)
    );
}

#[test]
fn picker_fit_strip_moves_back_across_its_direction() {
    // A horizontal strip can't get shorter vertically, so it moves up instead.
    assert_eq!(
        fit_strip(Rect::new(100, 1800, 1000, 320), AREA, false, MIN),
        Rect::new(100, 1644, 1000, 320)
    );
}

#[test]
fn picker_fit_strip_vertical_shrinks_its_height() {
    assert_eq!(
        fit_strip(Rect::new(100, 1000, 400, 1964), AREA, true, MIN),
        Rect::new(100, 1000, 400, 964)
    );
}

#[test]
fn picker_fit_strip_on_screen_is_unchanged() {
    let rect = Rect::new(500, 100, 2000, 320);
    assert_eq!(fit_strip(rect, AREA, false, MIN), rect);
}

#[test]
fn picker_fit_strip_minimum_longer_than_the_screen_is_the_screen() {
    assert_eq!(
        fit_strip(Rect::new(-50, 100, 5000, 320), AREA, false, 4000),
        Rect::new(0, 100, 3024, 320)
    );
}

#[test]
fn picker_resize_strip_end_edge() {
    let start = Rect::new(500, 100, 2000, 320);
    let resize = |delta| resize_strip(start, StripEnd::End, delta, AREA, false, MIN);
    assert_eq!(resize(300), Rect::new(500, 100, 2300, 320));
    // Stops at the screen edge.
    assert_eq!(resize(900), Rect::new(500, 100, 2524, 320));
    // Stops at the minimum length.
    assert_eq!(resize(-1500), Rect::new(500, 100, 640, 320));
}

#[test]
fn picker_resize_strip_start_edge_keeps_the_far_end() {
    let start = Rect::new(500, 100, 2000, 320);
    let resize = |delta| resize_strip(start, StripEnd::Start, delta, AREA, false, MIN);
    assert_eq!(resize(-200), Rect::new(300, 100, 2200, 320));
    assert_eq!(resize(-900), Rect::new(0, 100, 2500, 320));
    assert_eq!(resize(1500), Rect::new(1860, 100, 640, 320));
}

#[test]
fn picker_resize_strip_vertical_changes_the_height() {
    let start = Rect::new(100, 300, 400, 1000);
    assert_eq!(
        resize_strip(start, StripEnd::End, 200, AREA, true, MIN),
        Rect::new(100, 300, 400, 1200)
    );
    assert_eq!(
        resize_strip(start, StripEnd::Start, -100, AREA, true, MIN),
        Rect::new(100, 200, 400, 1100)
    );
}

#[test]
fn picker_fit_custom_uses_the_saved_spot_and_fits_it() {
    let mons = monitors();
    assert_eq!(
        fit_custom(
            &custom(2268, 1600, "Built-in Retina Display"),
            (3024, 320),
            &mons,
            false,
            MIN
        ),
        Ok(Rect::new(2268, 1600, 756, 320))
    );
    assert_eq!(
        fit_custom(
            &custom(0, 5000, "DELL U2720Q"),
            (2000, 320),
            &mons,
            false,
            MIN
        ),
        Err(Fallback::OffScreen)
    );
    assert_eq!(
        fit_custom(&custom(1, 1, "LG HDR 4K"), (2000, 320), &mons, false, MIN),
        Err(Fallback::MonitorMissing)
    );
}

const EDGES: [PickerPosition; 4] = [
    PickerPosition::Bottom,
    PickerPosition::Top,
    PickerPosition::Left,
    PickerPosition::Right,
];

#[test]
fn picker_place_edge_rect_matches_go_sizes_on_the_work_area() {
    let area = Rect::new(0, 50, 3024, 1890); // below a 50 px menu bar
    let s = 2.0;
    let auto = PickerOrientation::Auto;
    assert_eq!(
        edge_rect(PickerPosition::Bottom, auto, area, s),
        Rect::new(0, 1620, 3024, 320)
    );
    assert_eq!(
        edge_rect(PickerPosition::Top, auto, area, s),
        Rect::new(0, 50, 3024, 320)
    );
    assert_eq!(
        edge_rect(PickerPosition::Left, auto, area, s),
        Rect::new(0, 50, 400, 1890)
    );
    assert_eq!(
        edge_rect(PickerPosition::Right, auto, area, s),
        Rect::new(2624, 50, 400, 1890)
    );
    // Second monitor at a negative origin, scale 1.
    let area = Rect::new(-1920, -1080, 1920, 1080);
    assert_eq!(
        edge_rect(PickerPosition::Bottom, auto, area, 1.0),
        Rect::new(-1920, -160, 1920, 160)
    );
    // A tiny work area clamps instead of overflowing.
    let area = Rect::new(0, 0, 100, 100);
    assert_eq!(
        edge_rect(PickerPosition::Bottom, auto, area, 1.0),
        Rect::new(0, 0, 100, 100)
    );
}

#[test]
fn picker_place_on_axis_orientation_is_the_full_edge_strip() {
    let area = Rect::new(0, 50, 3024, 1890);
    for position in EDGES {
        let explicit = if matches!(position, PickerPosition::Left | PickerPosition::Right) {
            PickerOrientation::Vertical
        } else {
            PickerOrientation::Horizontal
        };
        for scale in [1.0, 2.0] {
            assert_eq!(
                edge_rect(position, explicit, area, scale),
                edge_rect(position, PickerOrientation::Auto, area, scale),
                "{position:?} {explicit:?} @{scale}"
            );
        }
    }
}

#[test]
fn picker_place_vertical_at_the_bottom_is_a_centred_docked_panel() {
    let v = PickerOrientation::Vertical;
    // Scale 1: 200 x 640, centred along the bottom edge.
    let area = Rect::new(0, 0, 1920, 1080);
    assert_eq!(
        edge_rect(PickerPosition::Bottom, v, area, 1.0),
        Rect::new(860, 440, 200, 640)
    );
    assert_eq!(
        edge_rect(PickerPosition::Top, v, area, 1.0),
        Rect::new(860, 0, 200, 640)
    );
    // Scale 2 below a menu bar: 400 x 1280 physical px.
    let area = Rect::new(0, 50, 3024, 1890);
    assert_eq!(
        edge_rect(PickerPosition::Bottom, v, area, 2.0),
        Rect::new(1312, 660, 400, 1280)
    );
    // Negative origin.
    let area = Rect::new(-1920, -1080, 1920, 1080);
    assert_eq!(
        edge_rect(PickerPosition::Bottom, v, area, 1.0),
        Rect::new(-1060, -640, 200, 640)
    );
}

#[test]
fn picker_place_horizontal_at_the_right_is_a_centred_docked_panel() {
    let h = PickerOrientation::Horizontal;
    // Scale 1: 1000 x 160, centred along the right edge.
    let area = Rect::new(0, 0, 1920, 1080);
    assert_eq!(
        edge_rect(PickerPosition::Right, h, area, 1.0),
        Rect::new(920, 460, 1000, 160)
    );
    assert_eq!(
        edge_rect(PickerPosition::Left, h, area, 1.0),
        Rect::new(0, 460, 1000, 160)
    );
    // Scale 2 below a menu bar: 2000 x 320 physical px.
    let area = Rect::new(0, 50, 3024, 1890);
    assert_eq!(
        edge_rect(PickerPosition::Right, h, area, 2.0),
        Rect::new(1024, 835, 2000, 320)
    );
}

#[test]
fn picker_place_off_axis_panel_clamps_to_a_small_work_area() {
    // Too short for 640 px: the vertical panel takes the full height.
    let area = Rect::new(0, 0, 800, 500);
    assert_eq!(
        edge_rect(
            PickerPosition::Bottom,
            PickerOrientation::Vertical,
            area,
            1.0
        ),
        Rect::new(300, 0, 200, 500)
    );
    // Too narrow for 1000 px: the horizontal panel takes the full width.
    assert_eq!(
        edge_rect(
            PickerPosition::Right,
            PickerOrientation::Horizontal,
            area,
            1.0
        ),
        Rect::new(0, 170, 800, 160)
    );
    // Scale 2: 1280 physical px wanted, 800 available.
    let area = Rect::new(100, 0, 1280, 800);
    assert_eq!(
        edge_rect(PickerPosition::Top, PickerOrientation::Vertical, area, 2.0),
        Rect::new(540, 0, 400, 800)
    );
    // A tiny work area never overflows.
    let area = Rect::new(0, 0, 100, 100);
    assert_eq!(
        edge_rect(
            PickerPosition::Left,
            PickerOrientation::Horizontal,
            area,
            1.0
        ),
        Rect::new(0, 0, 100, 100)
    );
}

#[test]
fn picker_place_wayland_size_follows_the_effective_orientation() {
    let auto = PickerOrientation::Auto;
    assert_eq!(
        floating_size(PickerPosition::Bottom, auto),
        FLOATING_HORIZONTAL
    );
    assert_eq!(
        floating_size(PickerPosition::Top, auto),
        FLOATING_HORIZONTAL
    );
    assert_eq!(floating_size(PickerPosition::Left, auto), FLOATING_VERTICAL);
    assert_eq!(
        floating_size(PickerPosition::Right, auto),
        FLOATING_VERTICAL
    );
    for position in EDGES {
        assert_eq!(
            floating_size(position, PickerOrientation::Horizontal),
            FLOATING_HORIZONTAL
        );
        assert_eq!(
            floating_size(position, PickerOrientation::Vertical),
            FLOATING_VERTICAL
        );
    }
    let (w, h) = FLOATING_HORIZONTAL;
    assert!(w > h);
    let (w, h) = FLOATING_VERTICAL;
    assert!(h > w);
}

// ── Platform selection ──────────────────────────────────────────────────────

fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |key| map.get(key).cloned()
}

#[test]
fn picker_place_kind_follows_os_and_compositor() {
    let none = env(&[]);
    assert_eq!(placer_kind("macos", &none), PlacerKind::Tauri);
    assert_eq!(placer_kind("windows", &none), PlacerKind::Tauri);
    assert_eq!(
        placer_kind("linux", &env(&[("DISPLAY", ":0")])),
        PlacerKind::Tauri
    );

    let hypr = env(&[
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("HYPRLAND_INSTANCE_SIGNATURE", "abc"),
    ]);
    assert_eq!(placer_kind("linux", &hypr), PlacerKind::Hyprland);
    let sway = env(&[
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("SWAYSOCK", "/run/user/1000/sway.sock"),
    ]);
    assert_eq!(placer_kind("linux", &sway), PlacerKind::Sway);
    let gnome = env(&[
        ("WAYLAND_DISPLAY", "wayland-0"),
        ("XDG_CURRENT_DESKTOP", "GNOME"),
    ]);
    assert_eq!(placer_kind("linux", &gnome), PlacerKind::Noop);

    // force_x11 relaunched us under XWayland: Tauri positions the window again.
    let xwayland = env(&[
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("HYPRLAND_INSTANCE_SIGNATURE", "abc"),
        ("GDK_BACKEND", "x11"),
    ]);
    assert_eq!(placer_kind("linux", &xwayland), PlacerKind::Tauri);
    // Empty values count as unset.
    assert_eq!(
        placer_kind("linux", &env(&[("WAYLAND_DISPLAY", "")])),
        PlacerKind::Tauri
    );
}

#[test]
fn picker_place_force_x11_relaunches_once_and_only_on_wayland() {
    let wayland = env(&[("WAYLAND_DISPLAY", "wayland-1")]);
    assert!(needs_x11_relaunch("linux", true, &wayland));
    assert!(
        !needs_x11_relaunch("linux", false, &wayland),
        "off by default"
    );
    assert!(!needs_x11_relaunch("macos", true, &wayland));
    assert!(
        !needs_x11_relaunch("linux", true, &env(&[("DISPLAY", ":0")])),
        "already X11"
    );
    let relaunched = env(&[
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("GDK_BACKEND", "x11"),
        ("KAKEMONO_UI_X11", "1"),
    ]);
    assert!(!needs_x11_relaunch("linux", true, &relaunched), "no loop");
    let marker_only = env(&[("WAYLAND_DISPLAY", "wayland-1"), ("KAKEMONO_UI_X11", "1")]);
    assert!(!needs_x11_relaunch("linux", true, &marker_only));
}

#[test]
fn picker_place_title_is_the_rule_key() {
    assert_eq!(PICKER_TITLE, "kakemono-picker");
}

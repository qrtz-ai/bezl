//! ADR 0200: "make the dock smaller" / "make the dock bigger" / "shrink the dock" end to end on Linux, through the
//! `ctrl:*` channels and the real ctrl helper protocol to the `FakeHelper`. One System Events script (a faceless
//! agent) moves the Dock's size one fixed step, the helper's read of the Dock's `tilesize` verifies it moved that way,
//! nothing is gated (it's reversible), no app is activated or hidden and the user's frontmost app / cursor / focus are
//! untouched. Already as small (or big) as it goes is done with nothing sent; without the System Events grant the run
//! asks and nothing runs; magnification, absolute sizes, mixed Dock tasks and other apps' sizes are not this.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{Consent, Route, ScriptVerb, WorkspaceOp};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::schedule::IanaZone;
use qrtz_ctrl::service::CtrlService;
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const T0: u64 = 1_790_000_000_000;

struct FakeClock {
    wall: AtomicU64,
    mono: AtomicU64,
}
impl Clock for FakeClock {
    fn mono_ns(&self) -> u64 {
        self.mono.fetch_add(1000, Ordering::SeqCst)
    }
    fn wall_ms(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
}
struct ClockHandle(Arc<FakeClock>);
impl Clock for ClockHandle {
    fn mono_ns(&self) -> u64 {
        self.0.mono_ns()
    }
    fn wall_ms(&self) -> u64 {
        self.0.wall_ms()
    }
}

fn rig(dir: &std::path::Path, w: FakeWorld) -> (CtrlService, Arc<FakeHelper>) {
    let fake = Arc::new(FakeHelper::new(w));
    let clock = Arc::new(FakeClock {
        wall: AtomicU64::new(T0),
        mono: AtomicU64::new(0),
    });
    let client = Arc::new(HelperClient::new(Box::new(fake.clone())));
    let s = CtrlService::open_with_zone(dir, Box::new(ClockHandle(clock)), Box::new(IanaZone(Los_Angeles)))
        .with_helper(client);
    (s, fake)
}

fn create(s: &CtrlService, text: &str) -> String {
    s.call(
        "ctrl:create-task",
        json!({"text": text, "schedule": {"kind": "once", "at": null}}),
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn run_now(s: &CtrlService, task: &str) -> Value {
    let r = s.call("ctrl:run-now", json!({"id": task})).unwrap();
    s.call("ctrl:run", json!({"runId": r["id"]})).unwrap()
}

fn world() -> FakeWorld {
    FakeWorld::typical(2)
}

fn granted() -> FakeWorld {
    let mut w = world();
    w.env
        .automation
        .insert("com.apple.systemevents".into(), Consent::Granted);
    w
}

fn log_of(d: &std::path::Path, v: &Value, t: &str, text: &str) -> String {
    let store = Store::open(d).unwrap();
    let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
    log.verify().unwrap();
    replay_check(&log, &Goal { task_id: t, text }).unwrap();
    log.to_jsonl().to_lowercase()
}

fn dock() -> FakeApp {
    FakeApp::new("com.apple.dock", "Dock", 90)
}

fn size_acts(w: &FakeWorld) -> usize {
    w.acted
        .iter()
        .filter(|a| {
            matches!(
                a.1,
                Route::Scripting {
                    verb: ScriptVerb::ShrinkDock | ScriptVerb::GrowDock,
                    ..
                }
            )
        })
        .count()
}

#[test]
fn make_the_dock_smaller_or_bigger_moves_its_size_one_step_through_system_events() {
    for (text, verb, bigger) in [
        ("make the dock smaller", ScriptVerb::ShrinkDock, false),
        ("make the dock a bit smaller", ScriptVerb::ShrinkDock, false),
        ("shrink the dock", ScriptVerb::ShrinkDock, false),
        ("make the dock icons smaller", ScriptVerb::ShrinkDock, false),
        ("decrease the dock size", ScriptVerb::ShrinkDock, false),
        ("dock smaller", ScriptVerb::ShrinkDock, false),
        (
            "every morning make the dock smaller",
            ScriptVerb::ShrinkDock,
            false,
        ),
        ("make the dock bigger", ScriptVerb::GrowDock, true),
        ("make the dock icons larger", ScriptVerb::GrowDock, true),
        ("increase the size of the dock", ScriptVerb::GrowDock, true),
        ("enlarge the dock", ScriptVerb::GrowDock, true),
        ("make my dock bigger", ScriptVerb::GrowDock, true),
    ] {
        for with_dock in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let mut w = granted();
            w.dock_tile = Some(48);
            if with_dock {
                w.apps.push(dock());
            }
            let (s, fake) = rig(d.path(), w);
            let before_w = fake.world();
            let t = create(&s, text);
            let v = run_now(&s, &t);
            assert_eq!(v["state"]["status"], "done", "{text}: {v}");
            assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
            assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.dock_size", "{text}");
            let w = fake.world();
            assert_eq!(
                w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
                vec![Route::Scripting {
                    app: "com.apple.systemevents".into(),
                    verb
                }],
                "{text}"
            );
            // One step of 0.15 on the 16–128 px range: 48 px → 65 px or 31 px.
            assert_eq!(w.dock_tile, Some(if bigger { 65 } else { 31 }), "{text}");
            assert_eq!(
                w.dock_autohide, before_w.dock_autohide,
                "{text}: autohide untouched"
            );
            assert_eq!(w.dock_edge, before_w.dock_edge, "{text}: edge untouched");
            assert_eq!(
                w.menu_bar_autohide, before_w.menu_bar_autohide,
                "{text}: menu bar untouched"
            );
            assert_eq!(w.dark, before_w.dark, "{text}: appearance untouched");
            assert_eq!(w.frontmost_pid, before_w.frontmost_pid);
            assert_eq!(w.cursor, before_w.cursor);
            assert_eq!(w.focused, before_w.focused);
            assert!(w
                .apps
                .iter()
                .zip(before_w.apps.iter())
                .all(|(a, b)| a.running == b.running && a.hidden == b.hidden));
            let raw = log_of(d.path(), &v, &t, text);
            assert!(raw.contains("route.scripting.dock_size_"), "{text}");
            assert!(
                raw.contains("\"dock_size\""),
                "{text}: verified by the size read back"
            );
            assert!(!raw.contains("the dock"), "{text}: task words in the log");
        }
    }
}

#[test]
fn already_as_small_or_as_big_as_it_goes_is_done_with_nothing_sent_and_no_grant_asked() {
    for (text, tile) in [("make the dock smaller", 16), ("make the dock bigger", 128)] {
        // No System Events grant: a task that can change nothing never asks for it.
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.dock_tile = Some(tile);
        let (s, fake) = rig(d.path(), w);
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        let w = fake.world();
        assert!(w.acted.is_empty(), "{text}: {:?}", w.acted);
        assert_eq!(w.dock_tile, Some(tile));
    }
    // One step from the end clamps to it and still verifies.
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.dock_tile = Some(20);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "make the dock smaller");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(fake.world().dock_tile, Some(16));
}

#[test]
fn a_mac_that_never_set_the_size_still_gets_the_step() {
    // `tilesize` absent: nothing to compare before, so the act stands unverified rather than being refused.
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.dock_tile = None;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "make the dock bigger");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(size_acts(&fake.world()), 1);
    assert!(fake.world().dock_tile.is_some());
}

#[test]
fn without_the_system_events_grant_it_asks_and_nothing_runs() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "make the dock smaller");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert!(v.to_string().contains("com.apple.systemevents"), "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert_eq!(w.dock_tile, Some(48));

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.env
        .automation
        .insert("com.apple.systemevents".into(), Consent::Denied);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "make the dock bigger");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty());
}

#[test]
fn a_script_that_didnt_take_fails_the_verify() {
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.no_effect = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "make the dock smaller");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert_eq!(fake.world().dock_tile, Some(48));
}

#[test]
fn both_ways_at_once_asks_and_nothing_runs() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), granted());
    let t = create(&s, "make the dock smaller or bigger");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert_eq!(size_acts(&fake.world()), 0, "{:?}", fake.world().acted);
}

#[test]
fn mixed_dock_tasks_ask_instead_of_doing_half() {
    for text in [
        "hide the dock and make it smaller",
        "move the dock to the left and make it bigger",
        "always show the dock and make it bigger",
    ] {
        for with_dock in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let mut w = granted();
            if with_dock {
                w.apps.push(dock());
            }
            let (s, fake) = rig(d.path(), w);
            let t = create(&s, text);
            let v = run_now(&s, &t);
            assert_ne!(v["state"]["status"], "done", "{text}: {v}");
            let w = fake.world();
            assert!(w.acted.is_empty(), "{text}: {:?}", w.acted);
            assert_eq!(w.dock_tile, Some(48), "{text}");
            assert_eq!(w.dock_edge, "bottom", "{text}");
            assert!(!w.dock_autohide, "{text}");
        }
    }
}

#[test]
fn other_size_words_and_dock_tasks_are_not_this() {
    for text in [
        "turn on dock magnification",
        "make the dock magnification bigger",
        "make the dock as small as possible",
        "make the dock the smallest",
        "set the dock size to 50",
        "reset the dock size",
        "make the dock text bigger",
        "make the slack window bigger",
        "make the menu bar bigger",
        "don't make the dock smaller",
        "keep the dock smaller",
        "toggle the dock size",
        "make the font bigger",
        "turn the volume up",
        "turn off the dock magnification",
        "what size is the dock",
    ] {
        for with_dock in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let mut w = granted();
            if with_dock {
                w.apps.push(dock());
            }
            let (s, fake) = rig(d.path(), w);
            let t = create(&s, text);
            let v = run_now(&s, &t);
            assert_ne!(
                v["state"]["decisions"][0]["chosen"], "intent.dock_size",
                "{text}: {v}"
            );
            let w = fake.world();
            assert_eq!(size_acts(&w), 0, "{text}: {:?}", w.acted);
            assert_eq!(w.dock_tile, Some(48), "{text}");
            assert!(!w.dock_autohide, "{text}: autohide untouched");
            assert_eq!(w.dock_edge, "bottom", "{text}: edge untouched");
            // Never a hidden launch or a hide of the Dock app.
            assert!(
                !w.acted.iter().any(|a| matches!(
                    &a.1,
                    Route::Workspace(WorkspaceOp::HideApps { apps } | WorkspaceOp::UnhideApps { apps })
                        if apps.iter().any(|x| x == "com.apple.dock")
                ) || matches!(&a.1, Route::Workspace(WorkspaceOp::LaunchHidden { app }) if app == "com.apple.dock")),
                "{text}: {:?}",
                w.acted
            );
        }
    }
    for (text, intent) in [
        ("hide the dock", "intent.dock_autohide"),
        ("always show the dock", "intent.dock_autohide"),
        ("move the dock to the left", "intent.dock_edge"),
        ("hide the menu bar", "intent.menu_bar_autohide"),
        ("turn the volume up", "intent.change_volume"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = rig(d.path(), granted());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["decisions"][0]["chosen"], intent, "{text}: {v}");
    }
}

#[test]
fn only_route_app_and_verb_cross_the_wire() {
    let d = tempfile::tempdir().unwrap();
    let (s, _) = rig(d.path(), granted());
    let t = create(&s, "make the dock smaller");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    let raw = log_of(d.path(), &v, &t, "make the dock smaller");
    assert!(raw.contains("dock_size_smaller"), "{raw}");
    assert!(raw.contains("shrink_dock"), "{raw}");
    assert!(!raw.contains("make the dock"), "{raw}");
    assert!(
        !raw.contains("0.15"),
        "{raw}: the step lives in the script template, not the wire"
    );
}

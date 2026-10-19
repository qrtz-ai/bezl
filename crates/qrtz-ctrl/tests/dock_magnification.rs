//! ADR 0202: "turn on dock magnification" / "turn off magnification on the dock" end to end on Linux, through the
//! `ctrl:*` channels and the real ctrl helper protocol to the `FakeHelper`. One System Events script (a faceless
//! agent) turns the Dock's magnification on or off, the helper's read of the Dock's `magnification` verifies it,
//! nothing is gated (it's reversible), no app is activated or hidden and the user's frontmost app / cursor / focus are
//! untouched. Already that way is done with nothing sent; without the System Events grant the run asks and nothing
//! runs; the magnification's size, toggles, negations, zoom and mixed Dock tasks are not this.
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

fn magnification_acts(w: &FakeWorld) -> usize {
    w.acted
        .iter()
        .filter(|a| {
            matches!(
                a.1,
                Route::Scripting {
                    verb: ScriptVerb::SetDockMagnificationOn | ScriptVerb::SetDockMagnificationOff,
                    ..
                }
            )
        })
        .count()
}

fn dock_app_touched(w: &FakeWorld) -> bool {
    w.acted.iter().any(|a| {
        matches!(
            &a.1,
            Route::Workspace(WorkspaceOp::HideApps { apps } | WorkspaceOp::UnhideApps { apps })
                if apps.iter().any(|x| x == "com.apple.dock")
        ) || matches!(&a.1, Route::Workspace(WorkspaceOp::LaunchHidden { app }) if app == "com.apple.dock")
    })
}

#[test]
fn turn_dock_magnification_on_or_off_sets_it_through_system_events() {
    for (text, on) in [
        ("turn on dock magnification", true),
        ("turn on magnification for the dock", true),
        ("enable dock magnification", true),
        ("turn dock magnification on", true),
        ("magnify the dock icons", true),
        ("always magnify the dock", true),
        ("every morning turn on dock magnification", true),
        ("keep dock magnification on", true),
        ("turn off dock magnification", false),
        ("turn off magnification on the dock", false),
        ("disable dock magnification", false),
        ("stop magnifying the dock icons", false),
        ("switch the dock magnification off", false),
        ("on mondays turn off dock magnification", false),
    ] {
        for with_dock in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let mut w = granted();
            w.dock_magnification = !on;
            if with_dock {
                w.apps.push(dock());
            }
            let (s, fake) = rig(d.path(), w);
            let before_w = fake.world();
            let t = create(&s, text);
            let v = run_now(&s, &t);
            assert_eq!(v["state"]["status"], "done", "{text}: {v}");
            assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
            assert_eq!(
                v["state"]["decisions"][0]["chosen"], "intent.dock_magnification",
                "{text}"
            );
            let w = fake.world();
            assert_eq!(
                w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
                vec![Route::Scripting {
                    app: "com.apple.systemevents".into(),
                    verb: if on {
                        ScriptVerb::SetDockMagnificationOn
                    } else {
                        ScriptVerb::SetDockMagnificationOff
                    }
                }],
                "{text}"
            );
            assert_eq!(w.dock_magnification, on, "{text}");
            assert_eq!(w.dock_tile, before_w.dock_tile, "{text}: size untouched");
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
            assert!(raw.contains("route.scripting.dock_magnification_"), "{text}");
            assert!(
                raw.contains("\"dock_magnification\""),
                "{text}: verified by the read back"
            );
            assert!(!raw.contains("the dock"), "{text}: task words in the log");
        }
    }
}

#[test]
fn already_that_way_is_done_with_nothing_sent_and_no_grant_asked() {
    for (text, on) in [
        ("turn on dock magnification", true),
        ("turn off dock magnification", false),
    ] {
        // No System Events grant: a task that changes nothing never asks for it.
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.dock_magnification = on;
        let (s, fake) = rig(d.path(), w);
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        let w = fake.world();
        assert!(w.acted.is_empty(), "{text}: {:?}", w.acted);
        assert_eq!(w.dock_magnification, on);
    }
}

#[test]
fn without_the_system_events_grant_it_asks_and_nothing_runs() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "turn on dock magnification");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert!(v.to_string().contains("com.apple.systemevents"), "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert!(!w.dock_magnification);

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.dock_magnification = true;
    w.env
        .automation
        .insert("com.apple.systemevents".into(), Consent::Denied);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "turn off dock magnification");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty());
    assert!(fake.world().dock_magnification);
}

#[test]
fn a_script_that_didnt_take_fails_the_verify() {
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.no_effect = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "turn on dock magnification");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(!fake.world().dock_magnification);
}

#[test]
fn no_way_or_both_ways_asks_and_nothing_runs() {
    for text in [
        "dock magnification",
        "turn dock magnification on or off",
        "enable or disable dock magnification",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), granted());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(v["state"]["status"], "done", "{text}: {v}");
        assert_eq!(
            magnification_acts(&fake.world()),
            0,
            "{text}: {:?}",
            fake.world().acted
        );
    }
}

#[test]
fn mixed_dock_tasks_ask_instead_of_doing_half() {
    for text in [
        "hide the dock and turn on magnification",
        "move the dock to the left and turn on magnification",
        "turn on dock magnification and make the dock smaller",
        "always show the dock and turn off magnification",
        "turn on dock magnification and hide the menu bar",
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
            assert!(!w.dock_magnification, "{text}");
            assert_eq!(w.dock_tile, Some(48), "{text}");
            assert_eq!(w.dock_edge, "bottom", "{text}");
            assert!(!w.dock_autohide, "{text}");
            assert!(!w.menu_bar_autohide, "{text}");
        }
    }
}

#[test]
fn magnification_size_toggles_negations_and_zoom_are_not_this() {
    for text in [
        "make the dock magnification bigger",
        "increase dock magnification",
        "more dock magnification",
        "set dock magnification to 50",
        "set the dock magnification size to max",
        "reset dock magnification",
        "toggle dock magnification",
        "don't magnify the dock",
        "never magnify the dock",
        "no dock magnification",
        "turn on dock zoom",
        "turn off zoom on the dock",
        "turn on magnification",
        "turn on screen magnification",
        "magnify the slack window",
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
            let chosen = v["state"]["decisions"][0]["chosen"].clone();
            for not in [
                "intent.dock_magnification",
                "intent.dock_autohide",
                "intent.dock_size",
                "intent.dock_edge",
                "intent.hide_app",
                "intent.show_app",
            ] {
                assert_ne!(chosen, not, "{text}: {v}");
            }
            let w = fake.world();
            assert_eq!(magnification_acts(&w), 0, "{text}: {:?}", w.acted);
            assert!(!w.dock_magnification, "{text}");
            assert_eq!(w.dock_tile, Some(48), "{text}: size untouched");
            assert!(!w.dock_autohide, "{text}: autohide untouched");
            assert_eq!(w.dock_edge, "bottom", "{text}: edge untouched");
            // Never a hidden launch or a hide of the Dock app.
            assert!(!dock_app_touched(&w), "{text}: {:?}", w.acted);
        }
    }
}

#[test]
fn the_other_dock_tasks_keep_their_intents() {
    for (text, intent) in [
        ("hide the dock", "intent.dock_autohide"),
        ("turn off dock hiding", "intent.dock_autohide"),
        ("always show the dock", "intent.dock_autohide"),
        ("move the dock to the left", "intent.dock_edge"),
        ("make the dock smaller", "intent.dock_size"),
        ("make the dock icons bigger", "intent.dock_size"),
        ("hide the menu bar", "intent.menu_bar_autohide"),
        ("turn the volume up", "intent.change_volume"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), granted());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["decisions"][0]["chosen"], intent, "{text}: {v}");
        assert_eq!(magnification_acts(&fake.world()), 0, "{text}");
    }
}

#[test]
fn only_route_app_and_verb_cross_the_wire() {
    let d = tempfile::tempdir().unwrap();
    let (s, _) = rig(d.path(), granted());
    let t = create(&s, "turn on dock magnification");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    let raw = log_of(d.path(), &v, &t, "turn on dock magnification");
    assert!(raw.contains("dock_magnification_on"), "{raw}");
    assert!(raw.contains("set_dock_magnification_on"), "{raw}");
    assert!(!raw.contains("turn on dock"), "{raw}");
    assert!(
        !raw.contains("magnification size"),
        "{raw}: its size is never named"
    );
}

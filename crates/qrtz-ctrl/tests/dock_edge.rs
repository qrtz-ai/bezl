//! ADR 0195: "move the dock to the left" / "put the dock on the right" / "dock on the bottom" end to end on Linux,
//! through the `ctrl:*` channels and the real ctrl helper protocol to the `FakeHelper`. One System Events script (a
//! faceless agent) sets the Dock's screen edge, the helper's read of the Dock's defaults verifies it, nothing is
//! gated (it's reversible), no app is activated or hidden and the user's frontmost app / cursor / focus are
//! untouched. Without the System Events Automation grant the run asks and nothing runs; hide/show dock stays
//! dock_autohide; other tasks keep their intents.
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

#[test]
fn move_the_dock_sets_its_screen_edge_through_system_events_and_verifies_it() {
    for (text, before, verb, after) in [
        (
            "move the dock to the left",
            "bottom",
            ScriptVerb::SetDockEdgeLeft,
            "left",
        ),
        (
            "put the dock on the left",
            "right",
            ScriptVerb::SetDockEdgeLeft,
            "left",
        ),
        ("dock on the left", "bottom", ScriptVerb::SetDockEdgeLeft, "left"),
        (
            "every morning move the dock to the left",
            "bottom",
            ScriptVerb::SetDockEdgeLeft,
            "left",
        ),
        (
            "put the dock on the right",
            "left",
            ScriptVerb::SetDockEdgeRight,
            "right",
        ),
        (
            "move the dock to the right",
            "bottom",
            ScriptVerb::SetDockEdgeRight,
            "right",
        ),
        ("dock on the right", "left", ScriptVerb::SetDockEdgeRight, "right"),
        (
            "dock on the bottom",
            "left",
            ScriptVerb::SetDockEdgeBottom,
            "bottom",
        ),
        (
            "move the dock to the bottom",
            "right",
            ScriptVerb::SetDockEdgeBottom,
            "bottom",
        ),
        (
            "put the dock on the bottom",
            "left",
            ScriptVerb::SetDockEdgeBottom,
            "bottom",
        ),
    ] {
        for with_dock in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let mut w = granted();
            w.dock_edge = before.into();
            if with_dock {
                w.apps.push(dock());
            }
            let (s, fake) = rig(d.path(), w);
            let before_w = fake.world();
            let t = create(&s, text);
            let v = run_now(&s, &t);
            assert_eq!(v["state"]["status"], "done", "{text}: {v}");
            assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
            assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.dock_edge", "{text}");
            let w = fake.world();
            assert_eq!(
                w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
                vec![Route::Scripting {
                    app: "com.apple.systemevents".into(),
                    verb
                }],
                "{text}"
            );
            assert_eq!(w.dock_edge, after, "{text}");
            assert_eq!(
                w.dock_autohide, before_w.dock_autohide,
                "{text}: autohide untouched"
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
            assert!(raw.contains("route.scripting.dock_edge_"), "{text}");
            assert!(!raw.contains("the dock"), "{text}: task words in the log");
        }
    }
}

#[test]
fn without_the_system_events_grant_it_asks_and_nothing_runs() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "move the dock to the left");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert!(v.to_string().contains("com.apple.systemevents"), "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert_eq!(w.dock_edge, "bottom");

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.env
        .automation
        .insert("com.apple.systemevents".into(), Consent::Denied);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "put the dock on the right");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty());
}

#[test]
fn a_script_that_didnt_take_fails_the_verify() {
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.no_effect = true;
    w.dock_edge = "bottom".into();
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "move the dock to the left");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert_eq!(fake.world().dock_edge, "bottom");
}

#[test]
fn hide_and_show_dock_stay_autohide_and_edge_words_dont_steal_other_tasks() {
    for text in [
        "hide the dock",
        "always show the dock",
        "auto-hide the dock",
        "stop hiding the dock",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = rig(d.path(), granted());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.dock_autohide",
            "{text}: {v}"
        );
        assert_ne!(v["state"]["decisions"][0]["chosen"], "intent.dock_edge", "{text}");
    }
    for text in [
        "hide slack",
        "show slack",
        "turn on dark mode",
        "mute my mac",
        "toggle the dock",
        "move slack to the left",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = rig(d.path(), granted());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(
            v["state"]["decisions"][0]["chosen"], "intent.dock_edge",
            "{text}: {v}"
        );
    }
    // With the Dock running, an edge task is never a hide of the Dock app.
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.apps.push(dock());
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "move the dock to the left");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.dock_edge", "{v}");
    assert!(!fake
        .world()
        .acted
        .iter()
        .any(|a| matches!(a.1, Route::Workspace(WorkspaceOp::HideApps { .. }))));
}

#[test]
fn two_edges_ask_and_nothing_runs() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), granted());
    let t = create(&s, "move the dock to the left or the right");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty(), "{:?}", fake.world().acted);
}

#[test]
fn only_route_app_and_verb_cross_the_wire() {
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.dock_edge = "bottom".into();
    let (s, _) = rig(d.path(), w);
    let t = create(&s, "move the dock to the left");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    let raw = log_of(d.path(), &v, &t, "move the dock to the left");
    assert!(raw.contains("dock_edge_left"), "{raw}");
    assert!(!raw.contains("move the dock"), "{raw}");
}

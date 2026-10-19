//! ADR 0173: "hide the dock" / "always show the dock" end to end on Linux, through the `ctrl:*` channels and the real
//! ctrl helper protocol to the `FakeHelper`. One System Events script (a faceless agent) sets the Dock's auto-hide,
//! the helper's read of the Dock's defaults verifies it, nothing is gated (it's reversible), no app is activated or
//! hidden (never `hide()` of the Dock itself) and the user's frontmost app / cursor / focus are untouched. Without
//! the System Events Automation grant the run asks and nothing runs; other tasks keep their intents.
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
fn hide_and_show_the_dock_set_its_autohide_through_system_events_and_verify_it() {
    for (text, before_hidden, verb, after_hidden) in [
        ("hide the dock", false, ScriptVerb::SetDockAutohide, true),
        ("auto-hide the dock", false, ScriptVerb::SetDockAutohide, true),
        ("turn on dock hiding", false, ScriptVerb::SetDockAutohide, true),
        (
            "automatically hide and show the dock",
            false,
            ScriptVerb::SetDockAutohide,
            true,
        ),
        (
            "every morning keep the dock hidden",
            false,
            ScriptVerb::SetDockAutohide,
            true,
        ),
        ("show the dock", true, ScriptVerb::SetDockShown, false),
        ("always show the dock", true, ScriptVerb::SetDockShown, false),
        ("stop hiding the dock", true, ScriptVerb::SetDockShown, false),
        ("turn off dock auto-hide", true, ScriptVerb::SetDockShown, false),
        ("don't hide the dock", true, ScriptVerb::SetDockShown, false),
        (
            "every friday keep the dock visible",
            true,
            ScriptVerb::SetDockShown,
            false,
        ),
    ] {
        // With and without the Dock in the running-app list: either way it's the auto-hide, never hiding the app.
        for with_dock in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let mut w = granted();
            w.dock_autohide = before_hidden;
            if with_dock {
                w.apps.push(dock());
            }
            let (s, fake) = rig(d.path(), w);
            let before = fake.world();
            let t = create(&s, text);
            let v = run_now(&s, &t);
            assert_eq!(v["state"]["status"], "done", "{text}: {v}");
            assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
            assert_eq!(
                v["state"]["decisions"][0]["chosen"], "intent.dock_autohide",
                "{text}"
            );
            let w = fake.world();
            assert_eq!(
                w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
                vec![Route::Scripting {
                    app: "com.apple.systemevents".into(),
                    verb
                }],
                "{text}"
            );
            assert_eq!(w.dock_autohide, after_hidden, "{text}");
            assert_eq!(w.dark, before.dark, "{text}: the appearance untouched");
            assert_eq!(w.frontmost_pid, before.frontmost_pid);
            assert_eq!(w.cursor, before.cursor);
            assert_eq!(w.focused, before.focused);
            assert!(w
                .apps
                .iter()
                .zip(before.apps.iter())
                .all(|(a, b)| a.running == b.running && a.hidden == b.hidden));
            let raw = log_of(d.path(), &v, &t, text);
            assert!(raw.contains("route.scripting.dock_autohide_"), "{text}");
            assert!(!raw.contains("the dock"), "{text}: task words in the log");
        }
    }
}

#[test]
fn without_the_system_events_grant_it_asks_and_nothing_runs() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "hide the dock");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert!(v.to_string().contains("com.apple.systemevents"), "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert!(!w.dock_autohide);

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.env
        .automation
        .insert("com.apple.systemevents".into(), Consent::Denied);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "always show the dock");
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
    let t = create(&s, "hide the dock");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(!fake.world().dock_autohide);
}

#[test]
fn dock_words_dont_steal_other_tasks_and_other_tasks_dont_take_the_dock() {
    // Hiding / showing an app, the appearance and a toggle (left out, ADR 0173) aren't the Dock's auto-hide.
    for text in [
        "hide slack",
        "show slack",
        "hide other apps",
        "turn on dark mode",
        "mute my mac",
        "toggle the dock",
        "move the dock to the left",
        "put the dock on the right",
        "dock on the bottom",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = rig(d.path(), granted());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(
            v["state"]["decisions"][0]["chosen"], "intent.dock_autohide",
            "{text}: {v}"
        );
    }
    // "hide the dock" with the Dock running is never a hide of the Dock app.
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.apps.push(dock());
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "hide the dock");
    let v = run_now(&s, &t);
    assert_eq!(
        v["state"]["decisions"][0]["chosen"], "intent.dock_autohide",
        "{v}"
    );
    assert!(!fake
        .world()
        .acted
        .iter()
        .any(|a| matches!(a.1, Route::Workspace(WorkspaceOp::HideApps { .. }))));
}

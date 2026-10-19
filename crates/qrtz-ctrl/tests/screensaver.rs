//! ADR 0178: "start the screensaver" end to end on Linux, through the `ctrl:*` channels and the real ctrl helper
//! protocol to the `FakeHelper`. One System Events script (a faceless agent) starts the current screen saver, the
//! helper's read of ScreenSaverEngine verifies it, nothing is gated (the user undoes it by moving the mouse), no app
//! is activated and the user's frontmost app / cursor / focus are untouched. Without the System Events Automation
//! grant the run asks and nothing runs; other tasks keep their intents. ADR 0178 amendment 1 (run 47): "stop the
//! screensaver" is the sibling `stop current screen saver`, verified by the engine being gone; a Mac that locks
//! after the screen saver keeps its password prompt (the verify fails, nothing is typed).
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{Consent, Route, ScriptVerb};
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

#[test]
fn start_the_screensaver_runs_through_system_events_and_verifies_it() {
    for text in [
        "start the screensaver",
        "start screensaver",
        "start the screen saver",
        "turn on the screensaver",
        "run the screen saver",
        "every night start the screensaver",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), granted());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.screensaver",
            "{text}"
        );
        let w = fake.world();
        assert_eq!(
            w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
            vec![Route::Scripting {
                app: "com.apple.systemevents".into(),
                verb: ScriptVerb::StartScreenSaver
            }],
            "{text}"
        );
        assert!(w.screensaver_running, "{text}");
        assert_eq!(w.dark, before.dark, "{text}: appearance untouched");
        assert_eq!(w.dock_autohide, before.dock_autohide, "{text}: dock untouched");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        let raw = log_of(d.path(), &v, &t, text);
        assert!(raw.contains("route.scripting.screensaver"), "{text}");
        assert!(
            !raw.contains("the screensaver") && !raw.contains("screen saver"),
            "{text}: task words in the log"
        );
    }
}

#[test]
fn already_running_is_done_and_a_script_that_didnt_take_fails_the_verify() {
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.screensaver_running = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "start the screensaver");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty(), "already up: nothing acted");

    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.no_effect = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "start the screensaver");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(!fake.world().screensaver_running);
}

#[test]
fn without_the_system_events_grant_it_asks_and_nothing_runs() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "start the screensaver");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert!(v.to_string().contains("com.apple.systemevents"), "{v}");
    assert!(fake.world().acted.is_empty());
    assert!(!fake.world().screensaver_running);

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.env
        .automation
        .insert("com.apple.systemevents".into(), Consent::Denied);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "start the screen saver");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty());
}

#[test]
fn lock_its_setting_and_other_tasks_are_not_this() {
    for text in [
        "lock the screen",
        "turn off the screensaver",
        "disable the screen saver",
        "never start the screensaver",
        "don't start the screensaver",
        "stop the screensaver from starting",
        "unlock the screensaver",
        "hide the dock",
        "turn on dark mode",
        "mute my mac",
        "start slack",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), granted());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(
            v["state"]["decisions"][0]["chosen"], "intent.screensaver",
            "{text}: {v}"
        );
        assert!(
            !fake.world().acted.iter().any(|a| matches!(
                a.1,
                Route::Scripting {
                    verb: ScriptVerb::StartScreenSaver | ScriptVerb::StopScreenSaver,
                    ..
                }
            )),
            "{text}: no screensaver script"
        );
    }
}

/// ADR 0178 amendment 1 (run 47): "stop the screensaver" is the sibling verb: one System Events script, no gate,
/// verified by ScreenSaverEngine being gone; the user's frontmost app / cursor / focus untouched.
#[test]
fn stop_the_screensaver_runs_through_system_events_and_verifies_it_is_gone() {
    for text in [
        "stop the screensaver",
        "stop screensaver",
        "quit the screen saver",
        "dismiss the screensaver",
        "end the screen saver",
        "stop the running screensaver",
        "every morning at 8 stop the screensaver",
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut w = granted();
        w.screensaver_running = true;
        let (s, fake) = rig(d.path(), w);
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}: no gate");
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.screensaver",
            "{text}"
        );
        let w = fake.world();
        assert_eq!(
            w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
            vec![Route::Scripting {
                app: "com.apple.systemevents".into(),
                verb: ScriptVerb::StopScreenSaver
            }],
            "{text}: one stop, never a start"
        );
        assert!(!w.screensaver_running, "{text}");
        assert_eq!(w.dark, before.dark, "{text}: appearance untouched");
        assert_eq!(w.dock_autohide, before.dock_autohide, "{text}: dock untouched");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        let raw = log_of(d.path(), &v, &t, text);
        assert!(raw.contains("route.scripting.screensaver_stop"), "{text}");
        assert!(
            !raw.contains("the screensaver") && !raw.contains("screen saver"),
            "{text}: task words in the log"
        );
    }
}

#[test]
fn stop_with_no_screensaver_up_is_done_and_a_locked_mac_fails_the_verify() {
    // Not running: done, nothing acted (the pre-check reads 0).
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), granted());
    let t = create(&s, "stop the screensaver");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty(), "nothing up: nothing acted");

    // A Mac that asks for a password after the screen saver keeps it up: the verify fails, nothing is typed or
    // unlocked, and the run doesn't say done.
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.screensaver_running = true;
    w.screensaver_locks = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "stop the screensaver");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    let w = fake.world();
    assert!(w.screensaver_running);
    assert!(
        w.acted.iter().all(|a| a.1
            == Route::Scripting {
                app: "com.apple.systemevents".into(),
                verb: ScriptVerb::StopScreenSaver
            }),
        "only the stop script, nothing typed: {:?}",
        w.acted
    );
}

#[test]
fn stop_without_the_system_events_grant_asks_and_nothing_runs() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.screensaver_running = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "stop the screensaver");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert!(v.to_string().contains("com.apple.systemevents"), "{v}");
    assert!(fake.world().acted.is_empty());
    assert!(fake.world().screensaver_running);
}

#[test]
fn the_stop_verb_goes_over_the_wire_as_stop_screen_saver_and_never_to_another_app() {
    let r = Route::Scripting {
        app: "com.apple.systemevents".into(),
        verb: ScriptVerb::StopScreenSaver,
    };
    let j = serde_json::to_value(&r).unwrap();
    assert_eq!(
        j,
        json!({"route": "scripting", "app": "com.apple.systemevents", "verb": "stop_screen_saver"})
    );
    assert_eq!(serde_json::from_value::<Route>(j).unwrap(), r);
    // The fake refuses it to anything but System Events, like the start.
    let mut w = granted();
    w.screensaver_running = true;
    let fake = FakeHelper::new(w);
    let client = HelperClient::new(Box::new(fake));
    let bad = Route::Scripting {
        app: "com.apple.ScreenSaver.Engine".into(),
        verb: ScriptVerb::StopScreenSaver,
    };
    assert!(client.act("a1", &bad, &Default::default()).is_err());
}

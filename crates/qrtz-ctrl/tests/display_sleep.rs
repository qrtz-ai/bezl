//! ADR 0180: "turn off my display" end to end on Linux, through the `ctrl:*` channels and the real ctrl helper
//! protocol to the `FakeHelper`. One `WorkspaceOp::SleepDisplay` in the helper's own process puts the displays to
//! sleep (the Mac stays awake), the helper's read of the main display verifies it, nothing is gated (any key wakes
//! it), no permission is asked, no app is activated and the user's frontmost app / cursor / focus are untouched.
//! Other screen tasks (screen saver, lock, Screen Time, sharing, brightness, keeping it awake) keep their intents.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{Route, WorkspaceOp};
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

fn log_of(d: &std::path::Path, v: &Value, t: &str, text: &str) -> String {
    let store = Store::open(d).unwrap();
    let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
    log.verify().unwrap();
    replay_check(&log, &Goal { task_id: t, text }).unwrap();
    log.to_jsonl().to_lowercase()
}

#[test]
fn turn_off_my_display_sleeps_it_in_the_helper_and_verifies_it() {
    for text in [
        "turn off my display",
        "turn off the screen",
        "put the display to sleep",
        "put my screen to sleep",
        "sleep the displays",
        "sleep display",
        "turn my monitor off",
        "switch off the monitors",
        "display off",
        "every night at 11 turn off my display",
        "every day at lunch time put the screen to sleep",
        "turn off the display on my mac",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.display_sleep",
            "{text}"
        );
        let w = fake.world();
        assert_eq!(
            w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
            vec![Route::Workspace(WorkspaceOp::SleepDisplay)],
            "{text}"
        );
        assert!(w.display_asleep, "{text}");
        assert!(!w.screensaver_running, "{text}: not the screen saver");
        assert_eq!(w.dark, before.dark, "{text}: appearance untouched");
        assert_eq!(w.muted, before.muted, "{text}: sound untouched");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        assert_eq!(
            w.env.automation, before.env.automation,
            "{text}: no Apple Event grant asked or used"
        );
        let raw = log_of(d.path(), &v, &t, text);
        assert!(raw.contains("route.system.display_sleep"), "{text}");
        for words in ["my display", "the screen", "to sleep", "monitor", "turn off"] {
            assert!(!raw.contains(words), "{text}: task words in the log ({words})");
        }
    }
}

#[test]
fn only_the_bare_op_crosses_the_wire() {
    assert_eq!(
        serde_json::to_value(Route::Workspace(WorkspaceOp::SleepDisplay)).unwrap(),
        json!({"route": "workspace", "op": "sleep_display"})
    );
    assert_eq!(
        serde_json::to_value(qrtz_ctrl::helper::Check::DisplayAsleep).unwrap(),
        json!({"check": "display_asleep"})
    );
}

#[test]
fn already_asleep_is_done_and_a_sleep_that_didnt_take_fails_the_verify() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.display_asleep = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "turn off my display");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty(), "already asleep: nothing acted");

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.no_effect = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "turn off my display");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(!fake.world().display_asleep);
}

#[test]
fn no_display_wrangler_is_not_done_and_nothing_else_is_tried() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.display_sleep_unavailable = true;
    let (s, fake) = rig(d.path(), w);
    let before = fake.world();
    let t = create(&s, "put the display to sleep");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    let w = fake.world();
    assert!(!w.display_asleep);
    assert!(!w.screensaver_running, "no fallback to the screen saver");
    assert!(
        w.acted
            .iter()
            .all(|a| a.1 == Route::Workspace(WorkspaceOp::SleepDisplay)),
        "no other route: {:?}",
        w.acted
    );
    assert_eq!(w.frontmost_pid, before.frontmost_pid);
}

#[test]
fn other_screen_tasks_are_not_this() {
    for text in [
        "put my mac to sleep",
        "lock the screen",
        "start the screensaver",
        "turn off the screen saver",
        "turn off screen time",
        "take a screenshot",
        "turn off screen sharing",
        "stop screen recording",
        "keep the display awake",
        "don't let the screen sleep",
        "turn off notifications on my screen",
        "dim the screen",
        "turn on the display",
        "mute my mac",
        "hide the dock",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(
            v["state"]["decisions"][0]["chosen"], "intent.display_sleep",
            "{text}: {v}"
        );
        assert!(!fake.world().display_asleep, "{text}");
    }
}

//! ADR 0183: "put my mac to sleep" end to end on Linux, through the `ctrl:*` channels and the real ctrl helper protocol
//! to the `FakeHelper`. One `WorkspaceOp::SleepSystem` in the helper's own process puts the whole Mac to sleep, the
//! helper's last sleep time moving forward verifies it, nothing is gated (any key, the mouse or the lid wakes it), no
//! permission is asked, no app is activated or quit, and the user's frontmost app / cursor / focus are untouched. While
//! the user is typing or clicking it defers (it never sleeps the Mac under their hands). The display alone (ADR 0180),
//! keeping it awake, its sleep settings, restart / shut down and lock keep their own intents.
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
fn put_my_mac_to_sleep_sleeps_it_in_the_helper_and_verifies_it() {
    for text in [
        "put my mac to sleep",
        "sleep my mac",
        "put the computer to sleep",
        "put my macbook to sleep",
        "sleep the laptop",
        "make my mac sleep",
        "send the mac to sleep",
        "every night at 11 put my mac to sleep",
        "every day at 6pm sleep the computer",
        "put the system to sleep",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.system_sleep",
            "{text}"
        );
        let w = fake.world();
        assert_eq!(
            w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
            vec![Route::Workspace(WorkspaceOp::SleepSystem)],
            "{text}"
        );
        assert_eq!(w.system_sleeps, 1, "{text}");
        assert!(w.last_sleep > before.last_sleep, "{text}");
        assert!(!w.display_asleep, "{text}: not the display-only move");
        assert!(!w.screensaver_running, "{text}: not the screen saver");
        assert_eq!(w.muted, before.muted, "{text}: sound untouched");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        assert_eq!(
            w.env.automation, before.env.automation,
            "{text}: no Apple Event grant asked or used"
        );
        let raw = log_of(d.path(), &v, &t, text);
        assert!(raw.contains("route.system.system_sleep"), "{text}");
        for words in ["my mac", "the computer", "to sleep", "laptop", "macbook"] {
            assert!(!raw.contains(words), "{text}: task words in the log ({words})");
        }
    }
}

#[test]
fn only_the_bare_op_crosses_the_wire() {
    assert_eq!(
        serde_json::to_value(Route::Workspace(WorkspaceOp::SleepSystem)).unwrap(),
        json!({"route": "workspace", "op": "sleep_system"})
    );
    assert_eq!(
        serde_json::to_value(qrtz_ctrl::helper::Check::LastSleep).unwrap(),
        json!({"check": "last_sleep"})
    );
}

#[test]
fn the_user_typing_right_now_defers_and_the_mac_stays_awake() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.env.user_idle_ms = Some(300);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "put my mac to sleep");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.system_sleep", "{v}");
    assert_eq!(v["state"]["status"], "deferred", "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert_eq!(w.system_sleeps, 0);
}

#[test]
fn a_refused_or_untaken_sleep_is_not_done_and_nothing_else_is_tried() {
    for (refused, no_effect) in [(true, false), (false, true)] {
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.system_sleep_refused = refused;
        w.no_effect = no_effect;
        let (s, fake) = rig(d.path(), w);
        let before = fake.world();
        let t = create(&s, "put my mac to sleep");
        let v = run_now(&s, &t);
        assert_ne!(
            v["state"]["status"], "done",
            "refused {refused} no_effect {no_effect}: {v}"
        );
        let w = fake.world();
        assert_eq!(w.system_sleeps, 0);
        assert_eq!(w.last_sleep, before.last_sleep);
        assert!(!w.display_asleep, "no fallback to the display");
        assert!(!w.screensaver_running, "no fallback to the screen saver");
        assert!(
            w.acted
                .iter()
                .all(|a| a.1 == Route::Workspace(WorkspaceOp::SleepSystem)),
            "no other route: {:?}",
            w.acted
        );
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
    }
}

#[test]
fn other_sleep_and_power_tasks_are_not_this() {
    for text in [
        "turn off my display",
        "put the screen to sleep",
        "keep my mac awake",
        "don't let my mac sleep",
        "never let the computer sleep",
        "change my mac's sleep settings",
        "set a sleep timer",
        "restart my mac",
        "shut down the computer",
        "lock my mac",
        "start the screensaver",
        "mute my mac",
        "wake my mac",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(
            v["state"]["decisions"][0]["chosen"], "intent.system_sleep",
            "{text}: {v}"
        );
        assert_eq!(fake.world().system_sleeps, 0, "{text}");
    }
}

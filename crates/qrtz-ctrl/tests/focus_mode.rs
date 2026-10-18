//! ADR 0155: "turn on do not disturb" / "turn off focus", end to end on Linux through the `ctrl:*` channels to the
//! `FakeHelper`. macOS has no public API to set a Focus, and the one background route (the Shortcuts CLI running a
//! shortcut the user made) isn't built, so the intent is recognised and its one move is a hold: the run is asked out
//! with `no-route` before anything is sent to the helper, the user's frontmost app, cursor and focus are untouched,
//! and the next scheduled run holds the same way instead of hanging on a question.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
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
    let mut w = FakeWorld::typical(2);
    w.watch_since_ms = Some(T0 - 60_000);
    w.day_start_ms = T0 - 3 * 3_600_000;
    w
}

#[test]
fn do_not_disturb_and_focus_are_recognised_and_asked_out_with_nothing_sent() {
    for text in [
        "turn on do not disturb",
        "turn off do not disturb",
        "dnd on",
        "turn on focus",
        "turn off focus mode",
        "every weekday at 9am turn on do not disturb",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        let st = &v["state"];
        assert_eq!(st["decisions"][0]["chosen"], "intent.focus_mode", "{text}: {v}");
        assert_eq!(st["status"], "asked-out", "{text}: {v}");
        assert_eq!(st["held"], "no-route", "{text}: {v}");
        assert!(st["ask"].is_null(), "{text}: no question left open: {v}");
        assert!(st["confirm"].is_null(), "{text}: no card");
        let w = fake.world();
        assert!(w.acted.is_empty(), "{text}: nothing sent: {:?}", w.acted);
        assert_eq!(w.frontmost_pid, before.frontmost_pid, "{text}");
        assert_eq!(w.cursor, before.cursor, "{text}");
        assert_eq!(w.focused, before.focused, "{text}");

        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        assert!(raw.contains("route.system.focus_hold"), "{text}");
        assert!(
            !raw.contains("action.requested"),
            "{text}: no action asked of the helper"
        );
        assert!(!raw.contains("cmd+"), "{text}: no key event planned");
        assert!(
            !raw.contains("disturb"),
            "{text}: the task's words stay out of the log"
        );

        // The chosen move is the hold, labelled in lowercase.
        let mv = st["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["node"] == "choose_move")
            .unwrap()
            .clone();
        assert_eq!(mv["chosen"], "route.system.focus_hold", "{mv}");
        let shown = mv["chosenLabel"].as_str().unwrap();
        assert_eq!(shown, "leave focus as it is, not supported yet");
        assert_eq!(shown, shown.to_lowercase());
    }
}

#[test]
fn a_scheduled_focus_task_holds_again_on_its_next_run() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "every weekday at 9am turn on do not disturb");
    for _ in 0..2 {
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "asked-out", "{v}");
        assert_eq!(v["state"]["held"], "no-route", "{v}");
    }
    assert!(fake.world().acted.is_empty());
}

#[test]
fn other_phrasings_keep_their_intents() {
    for (text, intent) in [
        ("mute my mac", "intent.mute"),
        ("turn on dark mode", "intent.dark_mode"),
        ("hide safari", "intent.hide_app"),
        ("hide other apps", "intent.hide_others"),
        ("turn the volume up", "intent.change_volume"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["decisions"][0]["chosen"], intent, "{text}: {v}");
    }
}

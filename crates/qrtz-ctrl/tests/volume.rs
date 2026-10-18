//! ADR 0140: "set the volume to N" end to end on Linux: the level comes from the task's own number (`level.N`),
//! one `SetVolume { level }` (which also unmutes) with no gate and no app touched, verified by reading the volume
//! back (exactly N, give or take 1); already at N is done; no output asks out; a time ("at 8") is never the level;
//! two numbers with neither clearly the level ask; no number, or an app named, isn't this.
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

fn muted() -> FakeWorld {
    let mut w = world();
    w.muted = Some(true);
    w
}

fn intent(v: &Value) -> &Value {
    &v["state"]["decisions"][0]["chosen"]
}

#[test]
fn set_the_volume_sets_that_level_unmutes_and_verifies_it_without_a_card() {
    for (text, level) in [
        ("set the volume to 30", 30u8),
        ("set volume to 30%", 30),
        ("every morning at 8 set the volume to 25", 25),
        ("every weekday at 9:30 set volume to 40 percent", 40),
        ("turn the sound to half", 50),
        ("volume 70", 70),
        ("set the volume to 0", 0),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), muted());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}");
        assert_eq!(intent(&v), "intent.set_volume", "{text}");
        let w = fake.world();
        assert_eq!(w.acted.len(), 1, "{text}");
        assert_eq!(
            w.acted[0].1,
            Route::Workspace(WorkspaceOp::SetVolume { level }),
            "{text}"
        );
        assert_eq!(w.volume, i64::from(level), "{text}");
        assert_eq!(w.muted, Some(false), "{text}: setting a level unmutes");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        assert!(raw.contains("route.system.set_volume"), "{text}");
        assert!(raw.contains(&format!("\"level.{level}\"")), "{text}");
        assert!(
            !raw.contains("the volume") && !raw.contains("percent") && !raw.contains("half"),
            "{text}: task words in the log"
        );
    }
}

#[test]
fn already_there_is_done_no_output_asks_out_and_unclear_levels_ask() {
    // Already at 50 and unmuted (the typical world): setting it again changes nothing and is done.
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "set the volume to 50");
    assert_eq!(run_now(&s, &t)["state"]["status"], "done");
    assert_eq!((fake.world().volume, fake.world().muted), (50, Some(false)));

    // No output device that has a volume.
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.muted = None;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "set the volume to 30");
    assert_eq!(run_now(&s, &t)["state"]["status"], "asked-out");
    assert_eq!(fake.world().volume, 50);

    // Two numbers, both after "to": not sure which, so it asks and sets nothing.
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), muted());
    let t = create(&s, "set the volume to 20 or to 40");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty(), "{v}");
    assert_eq!(fake.world().muted, Some(true));
}

#[test]
fn no_level_or_a_named_app_is_not_this_and_mute_unmute_are_unchanged() {
    for text in ["turn the volume back on", "unmute my mac"] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), muted());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(intent(&v), "intent.unmute", "{text}: {v}");
        assert_eq!(fake.world().volume, 50, "{text}");
    }
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "mute my mac");
    let v = run_now(&s, &t);
    assert_eq!(intent(&v), "intent.mute", "{v}");
    assert_eq!(fake.world().muted, Some(true));
    for text in ["set safari volume to 30", "every day at 8 check the volume"] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(intent(&v), "intent.set_volume", "{text}: {v}");
        assert_eq!(fake.world().volume, 50, "{text}");
    }
}

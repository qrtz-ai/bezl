//! ADR 0143: "turn it up / down" end to end on Linux: read the level first, step from it (default 10, "by N" or
//! "up N" overrides, "a bit" is 5), one `SetVolume` to that level with no gate and no app touched, verified against
//! the level read before; the ends of the range clamp and at the end is done; muted or no output asks out with
//! nothing changed; a sure level ("turn the volume down to 20") is still set_volume; an app named isn't this.
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

fn intent(v: &Value) -> &Value {
    &v["state"]["decisions"][0]["chosen"]
}

fn at(level: i64) -> FakeWorld {
    let mut w = world();
    w.volume = level;
    w.muted = Some(false);
    w
}

fn check_log(d: &std::path::Path, v: &Value, t: &str, text: &str) -> String {
    let store = Store::open(d).unwrap();
    let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
    log.verify().unwrap();
    replay_check(&log, &Goal { task_id: t, text }).unwrap();
    log.to_jsonl().to_lowercase()
}

#[test]
fn turn_it_up_or_down_steps_from_the_level_read_before_and_verifies_against_it() {
    for (text, from, to, mv) in [
        ("turn it up", 50i64, 60u8, "volume_up"),
        ("turn the volume down", 50, 40, "volume_down"),
        ("make it a bit louder", 50, 55, "volume_up"),
        ("turn the volume up by 25", 30, 55, "volume_up"),
        ("volume down 20%", 70, 50, "volume_down"),
        ("every evening at 9 turn the sound down", 35, 25, "volume_down"),
        ("lower the volume a little", 12, 7, "volume_down"),
        ("turn it up by 30", 85, 100, "volume_up"),
        ("turn it down", 4, 0, "volume_down"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), at(from));
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}");
        assert_eq!(intent(&v), "intent.change_volume", "{text}: {v}");
        let w = fake.world();
        assert_eq!(
            w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
            vec![Route::Workspace(WorkspaceOp::SetVolume { level: to })],
            "{text}"
        );
        assert_eq!(w.volume, i64::from(to), "{text}");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        let raw = check_log(d.path(), &v, &t, text);
        assert!(raw.contains(&format!("route.system.{mv}")), "{text}");
        assert!(raw.contains("\"step."), "{text}: the step is in the plan");
        assert!(
            raw.contains(&format!("\"before\":{from}")) && raw.contains(&format!("\"after\":{to}")),
            "{text}: verify against the read-before level: {raw}"
        );
        assert!(
            !raw.contains("louder") && !raw.contains("the volume") && !raw.contains("evening"),
            "{text}: task words in the log"
        );
    }
}

#[test]
fn at_the_end_is_done_muted_or_no_output_asks_out_and_nothing_changes() {
    // Already at the top / bottom: nothing to change, done, nothing acted.
    for (text, level) in [("turn it up", 100i64), ("turn the volume down", 0)] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), at(level));
        let t = create(&s, text);
        assert_eq!(run_now(&s, &t)["state"]["status"], "done", "{text}");
        assert!(fake.world().acted.is_empty(), "{text}");
        assert_eq!(fake.world().volume, level);
    }
    // Muted (no level to read) or no output: asks out, nothing set, still muted.
    for m in [Some(true), None] {
        let d = tempfile::tempdir().unwrap();
        let mut w = at(50);
        w.muted = m;
        let (s, fake) = rig(d.path(), w);
        let t = create(&s, "turn it up");
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "asked-out", "{m:?}: {v}");
        assert!(fake.world().acted.is_empty(), "{m:?}");
        assert_eq!((fake.world().volume, fake.world().muted), (50, m));
    }
}

#[test]
fn a_sure_level_is_still_set_volume_and_an_app_or_no_direction_is_not_this() {
    for (text, level) in [
        ("turn the volume down to 20", 20u8),
        ("lower the volume to 15", 15),
        ("turn the volume all the way up", 100),
        ("turn the sound all the way down", 0),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), at(50));
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(intent(&v), "intent.set_volume", "{text}: {v}");
        assert_eq!(fake.world().volume, i64::from(level), "{text}");
    }
    // "turn it down to 20" names a level but not the volume: it isn't a step (and set_volume wants the volume named).
    for text in [
        "turn up spotify",
        "every day at 8 check the volume",
        "turn up the brightness",
        "turn it down to 20",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), at(50));
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(intent(&v), "intent.change_volume", "{text}: {v}");
        assert_eq!(fake.world().volume, 50, "{text}");
    }
    // "mute my mac" / "unmute my mac" are unchanged by the new intent.
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), at(50));
    let t = create(&s, "mute my mac");
    assert_eq!(intent(&run_now(&s, &t)), "intent.mute");
    assert_eq!(fake.world().muted, Some(true));
}

//! ADR 0135: "mute my mac" end to end on Linux, through the `ctrl:*` channels and the real ctrl helper protocol to
//! the `FakeHelper`. The intent layers pick it with no question, one `MuteOutput` route mutes the sound output with no
//! gate (it's reversible) and no app touched, the helper's read-back verifies it, the user's frontmost app / cursor /
//! focus are untouched, and a Mac with no output that can mute asks out.
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

#[test]
fn mute_my_mac_mutes_the_output_without_a_card_and_verifies_it() {
    for text in [
        "mute my mac",
        "every night at 10 mute my mac",
        "mute the sound",
        "silence my computer",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
        assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.mute", "{text}");
        let w = fake.world();
        assert_eq!(w.acted.len(), 1, "{text}");
        assert_eq!(w.acted[0].1, Route::Workspace(WorkspaceOp::MuteOutput));
        assert_eq!(w.muted, Some(true), "{text}");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        assert!(w
            .apps
            .iter()
            .zip(before.apps.iter())
            .all(|(a, b)| a.running == b.running && a.hidden == b.hidden));
        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        assert!(raw.contains("route.system.mute"), "{text}");
        assert!(
            !raw.contains("my mac") && !raw.contains("silence"),
            "{text}: task words in the log"
        );
    }
}

#[test]
fn already_muted_is_done_no_output_asks_out_and_mute_slack_is_not_this() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.muted = Some(true);
    let (s, _) = rig(d.path(), w);
    let t = create(&s, "mute my mac");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.muted = None;
    let (s, _) = rig(d.path(), w);
    let t = create(&s, "mute my mac");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");

    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "mute safari");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["decisions"][0]["chosen"], "intent.mute", "{v}");
    assert_eq!(fake.world().muted, Some(false));
}

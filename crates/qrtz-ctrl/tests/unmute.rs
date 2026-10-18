//! ADR 0139: "unmute my mac" end to end on Linux, the reverse of ADR 0135's mute: one `UnmuteOutput` with no gate
//! and no app touched, verified by reading the mute back (expect 0); an unmuted Mac is done; no output asks out;
//! "mute my mac" still mutes and "unmute slack" isn't this.
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

#[test]
fn unmute_my_mac_turns_the_sound_back_on_without_a_card_and_verifies_it() {
    for text in [
        "unmute my mac",
        "every morning at 8 unmute my mac",
        "turn the sound back on",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), muted());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}");
        assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.unmute", "{text}");
        let w = fake.world();
        assert_eq!(w.acted.len(), 1, "{text}");
        assert_eq!(w.acted[0].1, Route::Workspace(WorkspaceOp::UnmuteOutput));
        assert_eq!(w.muted, Some(false), "{text}");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        assert!(raw.contains("route.system.unmute"), "{text}");
        assert!(
            !raw.contains("my mac") && !raw.contains("sound back"),
            "{text}: task words in the log"
        );
    }
}

#[test]
fn unmuted_is_done_no_output_asks_out_and_mute_still_mutes() {
    let d = tempfile::tempdir().unwrap();
    let (s, _) = rig(d.path(), world());
    let t = create(&s, "unmute my mac");
    assert_eq!(run_now(&s, &t)["state"]["status"], "done");

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.muted = None;
    let (s, _) = rig(d.path(), w);
    let t = create(&s, "unmute my mac");
    assert_eq!(run_now(&s, &t)["state"]["status"], "asked-out");

    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "mute my mac");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.mute", "{v}");
    assert_eq!(fake.world().muted, Some(true));

    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), muted());
    let t = create(&s, "unmute safari");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["decisions"][0]["chosen"], "intent.unmute", "{v}");
    assert_eq!(fake.world().muted, Some(true));
}

//! ADR 0176: "eject my disks" end to end on Linux: one `EjectVolumes` in the helper's own process with no gate, no
//! app and no key event, verified by no ejectable volume left mounted (expect 0). The helper picks the volumes; a
//! singular task ejects only while exactly one is mounted and otherwise asks out; nothing mounted is done with nothing
//! acted; a busy volume is never forced; nothing from the task crosses to the helper; other tasks keep their intents.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient, Wire};
use qrtz_ctrl::ladder::{Route, WorkspaceOp};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::schedule::IanaZone;
use qrtz_ctrl::service::CtrlService;
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

/// Every line the host sends.
struct Recording {
    inner: Arc<FakeHelper>,
    lines: Arc<Mutex<Vec<String>>>,
}
impl Wire for Recording {
    fn roundtrip(&self, line: &str) -> Result<String, String> {
        self.lines.lock().unwrap().push(line.to_string());
        self.inner.roundtrip(line)
    }
}

#[test]
fn eject_my_disks_ejects_every_one_without_a_card_and_verifies_it() {
    for (text, single, mounted) in [
        ("eject my disks", false, 2),
        ("eject all drives", false, 3),
        ("every day at 6pm eject all my drives", false, 2),
        ("unmount everything", false, 1),
        ("eject every disk before i leave", false, 2),
        ("eject my disks in finder", false, 2),
        ("eject the usb drive", true, 1),
        ("every day eject my sd card", true, 1),
        ("unmount the external disk", true, 1),
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.volumes = Some(vec![false; mounted]);
        let (s, fake) = rig(d.path(), w);
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}");
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.eject_disks",
            "{text}"
        );
        let w = fake.world();
        assert_eq!(w.acted.len(), 1, "{text}");
        assert_eq!(
            w.acted[0].1,
            Route::Workspace(WorkspaceOp::EjectVolumes { single }),
            "{text}"
        );
        assert_eq!(w.volumes, Some(vec![]), "{text}");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        assert!(w.apps.iter().all(|a| a.running
            == before
                .apps
                .iter()
                .find(|b| b.bundle_id == a.bundle_id)
                .unwrap()
                .running));
        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        let mv = if single {
            "route.system.eject_one"
        } else {
            "route.system.eject_all"
        };
        assert!(raw.contains(mv), "{text}");
        assert!(
            !raw.contains("my disks") && !raw.contains("usb drive"),
            "{text}: task words in the log"
        );
    }
}

#[test]
fn nothing_mounted_is_done_with_nothing_ejected() {
    for text in ["eject my disks", "eject the usb drive"] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(fake.world().acted.is_empty(), "{text}");
    }
}

#[test]
fn a_singular_task_with_two_disks_or_no_list_asks_out_and_ejects_nothing() {
    for volumes in [Some(vec![false, false]), None] {
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.volumes = volumes.clone();
        let (s, fake) = rig(d.path(), w);
        let t = create(&s, "eject the usb drive");
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.eject_disks", "{v}");
        assert_eq!(v["state"]["status"], "asked-out", "{volumes:?}: {v}");
        assert_eq!(v["state"]["held"], "no-route");
        let w = fake.world();
        assert!(w.acted.is_empty(), "{:?}", w.acted);
        assert_eq!(w.volumes, volumes);
    }
    // The helper refuses a singular eject itself when the count moved between the read and the act.
    let mut w = world();
    w.volumes = Some(vec![false, false]);
    let fake = FakeHelper::new(w);
    let r = fake
        .roundtrip(r#"{"type":"ctrl.act","id":"x","actionId":"a","route":{"route":"workspace","op":"eject_volumes","single":true},"content":{}}"#)
        .unwrap();
    let r: Value = serde_json::from_str(&r).unwrap();
    assert_eq!(r["ok"], false, "{r}");
    assert_eq!(r["err"], "volumes.not_one");
    assert_eq!(fake.world().volumes, Some(vec![false, false]));
}

#[test]
fn a_busy_disk_is_never_forced_and_a_no_effect_eject_fails_the_verify() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.volumes = Some(vec![false, true, false]);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "eject my disks");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert_eq!(
        fake.world().volumes,
        Some(vec![true]),
        "the busy one stays mounted"
    );

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.volumes = Some(vec![false]);
    w.no_effect = true;
    let (s, _) = rig(d.path(), w);
    let t = create(&s, "eject my disks");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
}

#[test]
fn nothing_from_the_task_crosses_the_wire() {
    let text = "eject my drives, the one called tax returns 2025";
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.volumes = Some(vec![false, false]);
    let fake = Arc::new(FakeHelper::new(w));
    let lines = Arc::new(Mutex::new(Vec::new()));
    let clock = Arc::new(FakeClock {
        wall: AtomicU64::new(T0),
        mono: AtomicU64::new(0),
    });
    let client = Arc::new(HelperClient::new(Box::new(Recording {
        inner: fake.clone(),
        lines: lines.clone(),
    })));
    let s = CtrlService::open_with_zone(
        d.path(),
        Box::new(ClockHandle(clock)),
        Box::new(IanaZone(Los_Angeles)),
    )
    .with_helper(client);
    let t = create(&s, text);
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    let lines = lines.lock().unwrap();
    let acts: Vec<&String> = lines.iter().filter(|l| l.contains("\"ctrl.act\"")).collect();
    assert_eq!(acts.len(), 1, "{acts:?}");
    let act: Value = serde_json::from_str(acts[0]).unwrap();
    assert_eq!(
        act["route"],
        json!({"route": "workspace", "op": "eject_volumes", "single": false})
    );
    assert_eq!(act["content"], json!({}));
    let measures: Vec<&String> = lines.iter().filter(|l| l.contains("ejectable_volumes")).collect();
    assert!(
        measures.len() >= 2,
        "the pre-check and the verify, a count only: {measures:?}"
    );
    assert!(
        measures.iter().all(|l| l.contains("\"ctrl.measure\"")),
        "{measures:?}"
    );
    for l in lines.iter() {
        for word in ["tax", "returns", "2025"] {
            assert!(!l.to_lowercase().contains(word), "{word} reached the helper: {l}");
        }
    }
}

#[test]
fn other_tasks_keep_their_intents_and_eject_nothing() {
    for (text, intent) in [
        ("empty the trash", "intent.empty_trash"),
        ("quit finder", "intent.quit_app"),
        ("open my downloads", "intent.open_folder"),
        ("hide the dock", "intent.dock_autohide"),
        ("clean up my downloads folder", "intent.clean_folder"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.volumes = Some(vec![false]);
        let (s, fake) = rig(d.path(), w);
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["decisions"][0]["chosen"], intent, "{text}: {v}");
        assert_eq!(fake.world().volumes, Some(vec![false]), "{text}");
    }
}

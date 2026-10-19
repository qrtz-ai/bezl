//! ADR 0168: "clear my clipboard" end to end on Linux: one `ClearClipboard` in the helper's own process with no gate,
//! no app and no key event, verified by the pasteboard's change count (expect 1); the user typing or clicking right
//! now defers it (never wipe a copy they're about to paste); no pasteboard fails; nothing from the clipboard or the
//! task's words crosses to the helper; "clear my downloads" and "empty the trash" keep their intents.
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
fn clear_my_clipboard_empties_it_without_a_card_and_verifies_it() {
    for text in [
        "clear my clipboard",
        "every hour clear my clipboard",
        "empty the clipboard",
        "wipe the clipboard every night at 11pm",
        "erase what i copied",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}");
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.clear_clipboard",
            "{text}"
        );
        let w = fake.world();
        assert_eq!(w.acted.len(), 1, "{text}");
        assert_eq!(w.acted[0].1, Route::Workspace(WorkspaceOp::ClearClipboard));
        assert_eq!(w.clipboard, Some(false), "{text}");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        assert!(raw.contains("route.system.clear_clipboard"), "{text}");
        assert!(
            !raw.contains("my clipboard") && !raw.contains("copied"),
            "{text}: task words in the log"
        );
    }
}

#[test]
fn the_user_typing_right_now_defers_and_nothing_is_cleared() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.env.user_idle_ms = Some(300);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "clear my clipboard");
    let v = run_now(&s, &t);
    assert_eq!(
        v["state"]["decisions"][0]["chosen"], "intent.clear_clipboard",
        "{v}"
    );
    assert_eq!(v["state"]["status"], "deferred", "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert_eq!(w.clipboard, Some(true));
}

#[test]
fn no_pasteboard_fails_and_a_copy_since_fails_the_verify() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.clipboard = None;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "clear my clipboard");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert_eq!(fake.world().clipboard, None);

    // The helper says the clear took, but the pasteboard moved on (the fake's clear does nothing): not verified.
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.no_effect = true;
    let (s, _) = rig(d.path(), w);
    let t = create(&s, "clear my clipboard");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
}

#[test]
fn nothing_from_the_clipboard_or_the_task_crosses_the_wire() {
    let text = "clear my clipboard, it has my bank password hunter2";
    let d = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeHelper::new(world()));
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
        json!({"route": "workspace", "op": "clear_clipboard"})
    );
    assert_eq!(act["content"], json!({}));
    let measures: Vec<&String> = lines.iter().filter(|l| l.contains("clipboard_cleared")).collect();
    assert_eq!(
        measures.len(),
        2,
        "the verify's before and after reads, the change count only"
    );
    assert!(
        measures.iter().all(|l| l.contains("\"ctrl.measure\"")),
        "{measures:?}"
    );
    for l in lines.iter() {
        for word in ["bank", "password", "hunter2"] {
            assert!(!l.to_lowercase().contains(word), "{word} reached the helper: {l}");
        }
    }
}

#[test]
fn other_clearing_and_emptying_tasks_keep_their_intents() {
    for (text, intent) in [
        ("clear my downloads", "intent.clean_folder"),
        ("empty the trash", "intent.empty_trash"),
        ("mute my mac", "intent.mute"),
        ("open my downloads", "intent.open_folder"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["decisions"][0]["chosen"], intent, "{text}: {v}");
        assert_eq!(fake.world().clipboard, Some(true), "{text}");
    }
}

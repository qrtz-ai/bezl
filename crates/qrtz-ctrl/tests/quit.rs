//! ADR 0130: "quit apps i haven't used today" end to end on Linux, through the `ctrl:*` channels and the real ctrl
//! helper protocol to the `FakeHelper`. The helper's last-used record decides (never the screen), the cold-start
//! rule keeps apps ctrl never watched, the frontmost app / Finder / qrtz are never quit, the quit waits at the ADR
//! 0111 gate with an apps preview (names hashed in the log), deny never acts, approve terminates exactly the listed
//! apps once with the user's frontmost app, cursor and focus untouched, and the log replays with no task words.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{Route, WorkspaceOp};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::schedule::IanaZone;
use qrtz_ctrl::service::CtrlService;
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const T0: u64 = 1_790_000_000_000;
const MIDNIGHT: u64 = T0 - 3 * 3_600_000;

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

/// Notes frontmost (never activated today per the record, still kept), Safari and Music used today, Slack last used
/// yesterday, TextEdit never activated while watched (one unsaved document), Finder and qrtz running.
fn world(since: u64) -> FakeWorld {
    let mut w = FakeWorld::typical(2);
    w.apps
        .push(FakeApp::new("com.tinyspeck.slackmacgap", "Slack", 500));
    let mut te = FakeApp::new("com.apple.TextEdit", "TextEdit", 400);
    te.docs = vec![("Secret plan.txt".into(), true)];
    w.apps.push(te);
    w.apps.push(FakeApp::new("com.apple.Music", "Music", 600));
    w.apps.push(FakeApp::new("ai.qrtz.desktop", "qrtz", 700));
    w.last_active.insert("com.apple.Safari".into(), MIDNIGHT + 60_000);
    w.last_active.insert("com.apple.Music".into(), MIDNIGHT + 120_000);
    w.last_active
        .insert("com.tinyspeck.slackmacgap".into(), MIDNIGHT - 3_600_000);
    w.watch_since_ms = Some(since);
    w.day_start_ms = MIDNIGHT;
    w
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

fn confirm(s: &CtrlService, v: &Value, answer: &str) -> Value {
    s.call(
        "ctrl:confirm",
        json!({"runId": v["run"]["id"], "gateId": v["state"]["confirm"]["gateId"], "answer": answer}),
    )
    .unwrap()
}

fn running(f: &FakeHelper, id: &str) -> bool {
    f.world().apps.iter().any(|a| a.bundle_id == id && a.running)
}

const WATCHED_ALL_DAY: u64 = MIDNIGHT - 10 * 3_600_000;

#[test]
fn quit_unused_apps_waits_at_the_gate_with_the_apps_then_quits_exactly_those() {
    for text in [
        "quit apps i haven't used today",
        "quit unused apps",
        "every evening close apps i'm not using",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world(WATCHED_ALL_DAY));
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        let st = &v["state"];
        assert_eq!(st["status"], "needs-confirm", "{text}: {v}");
        let c = &st["confirm"];
        assert_eq!(c["action"], "quit apps you haven't used today");
        assert_eq!(
            c["rules"],
            json!([
                "choose_intent:intent.quit_unused",
                "choose_move:route.workspace.quit_unused"
            ])
        );
        let items: Vec<(String, String, Value)> = c["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["kind"].as_str().unwrap().to_string(),
                    i["label"].as_str().unwrap().to_string(),
                    i["unsaved"].clone(),
                )
            })
            .collect();
        assert_eq!(
            items,
            vec![
                ("app".into(), "slack".into(), Value::Null),
                ("app".into(), "textedit".into(), json!(true)),
            ],
            "{text}"
        );
        assert!(fake.world().acted.is_empty(), "nothing quits before the answer");

        let v = confirm(&s, &v, "approve");
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        let w = fake.world();
        assert_eq!(w.acted.len(), 1, "one action");
        assert_eq!(
            w.acted[0].1,
            Route::Workspace(WorkspaceOp::QuitApps {
                apps: vec!["com.tinyspeck.slackmacgap".into(), "com.apple.TextEdit".into()]
            })
        );
        assert!(!running(&fake, "com.tinyspeck.slackmacgap"));
        assert!(!running(&fake, "com.apple.TextEdit"));
        for kept in [
            "com.apple.Notes",
            "com.apple.Safari",
            "com.apple.Music",
            "com.apple.finder",
            "ai.qrtz.desktop",
        ] {
            assert!(running(&fake, kept), "{kept} kept");
        }
        // Guard fields unchanged: the user's frontmost app, cursor and focus.
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);

        // The log replays exactly and holds no task words and no app names.
        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        for word in [
            "haven't",
            "unused apps",
            "not using",
            "secret",
            "\"slack\"",
            "\"textedit\"",
        ] {
            assert!(!raw.contains(word), "{word} reached the log");
        }
        drop(store);
        for e in walkdir(d.path()) {
            let body = std::fs::read_to_string(&e).unwrap_or_default().to_lowercase();
            if e.components().any(|c| c.as_os_str() == "runs") {
                assert!(
                    !body.contains("haven't used")
                        && !body.contains("unused apps")
                        && !body.contains("secret"),
                    "task words in a run file: {e:?}"
                );
            }
        }
    }
}

fn walkdir(p: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(p).unwrap().flatten() {
        let path = e.path();
        if path.is_dir() {
            out.extend(walkdir(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn deny_never_quits_and_cold_start_quits_nothing() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world(WATCHED_ALL_DAY));
    let t = create(&s, "quit unused apps");
    let v = run_now(&s, &t);
    let v = confirm(&s, &v, "deny");
    assert_eq!(v["state"]["status"], "cancelled", "{v}");
    assert!(fake.world().acted.is_empty());
    assert!(running(&fake, "com.tinyspeck.slackmacgap"));

    // The helper began watching today: only Slack (seen before midnight? no: the record can't predate the watch)
    // would count, and only a full day's watch does; nothing is quit and nothing is asked.
    let d = tempfile::tempdir().unwrap();
    let mut w = world(MIDNIGHT + 1_000);
    w.last_active.remove("com.tinyspeck.slackmacgap");
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "quit unused apps");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(v["state"]["confirm"].is_null(), "no gate when nothing qualifies");
    assert!(fake.world().acted.is_empty());

    // A helper that doesn't watch: asks out, nothing quit.
    let d = tempfile::tempdir().unwrap();
    let mut w = world(WATCHED_ALL_DAY);
    w.watch_since_ms = None;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "quit unused apps");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert_eq!(v["state"]["held"], "no-route");
    assert!(fake.world().acted.is_empty());
}

#[test]
fn an_app_used_between_the_answer_and_the_act_is_kept() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world(WATCHED_ALL_DAY));
    let t = create(&s, "quit unused apps");
    let v = run_now(&s, &t);
    // The user switches to Slack before approving: it's used today now, so it drops out; TextEdit still quits.
    fake.0
        .lock()
        .unwrap()
        .last_active
        .insert("com.tinyspeck.slackmacgap".into(), T0);
    let v = confirm(&s, &v, "approve");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(running(&fake, "com.tinyspeck.slackmacgap"));
    assert!(!running(&fake, "com.apple.TextEdit"));

    // "quit slack" stays the single-app quit.
    let d = tempfile::tempdir().unwrap();
    let (s, _) = rig(d.path(), world(WATCHED_ALL_DAY));
    let t = create(&s, "quit slack");
    let v = run_now(&s, &t);
    assert_eq!(
        v["state"]["confirm"]["rules"][0], "choose_intent:intent.quit_app",
        "{v}"
    );
}

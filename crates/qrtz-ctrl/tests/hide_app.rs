//! ADR 0149: "hide <app>" for one named app, end to end on Linux through the `ctrl:*` channels and the ctrl helper
//! protocol to the `FakeHelper`. One `NSRunningApplication.hide()` of that app (no key event, no activation, no
//! gate: it's reversible), verified from `ctrl.apps`; the app the user is in is never hidden (that would bring another
//! app forward), and an app that isn't running or is hidden already is done with nothing acted.
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

/// Notes frontmost, Safari, Finder, Slack, TextEdit (already hidden) and qrtz running; Chrome not running.
fn world() -> FakeWorld {
    let mut w = FakeWorld::typical(2);
    w.apps
        .push(FakeApp::new("com.tinyspeck.slackmacgap", "Slack", 500));
    let mut te = FakeApp::new("com.apple.TextEdit", "TextEdit", 400);
    te.hidden = true;
    w.apps.push(te);
    w.apps.push(FakeApp::new("ai.qrtz.desktop", "qrtz", 700));
    w.watch_since_ms = Some(T0 - 60_000);
    w.day_start_ms = MIDNIGHT;
    w
}

fn hidden(f: &FakeHelper, id: &str) -> bool {
    f.world().apps.iter().any(|a| a.bundle_id == id && a.hidden)
}

const SLACK: &str = "com.tinyspeck.slackmacgap";

#[test]
fn hide_a_named_app_hides_only_that_app_without_a_gate_key_or_activation() {
    for text in ["hide slack", "hide slack every evening"] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        let st = &v["state"];
        assert_eq!(st["status"], "done", "{text}: {v}");
        assert!(st["confirm"].is_null(), "{text}: reversible, no card");
        let w = fake.world();
        let routes: Vec<&Route> = w.acted.iter().map(|(_, r)| r).collect();
        assert_eq!(
            routes,
            vec![&Route::Workspace(WorkspaceOp::HideApps {
                apps: vec![SLACK.to_string()]
            })],
            "{text}: exactly one workspace hide of slack"
        );
        assert!(hidden(&fake, SLACK), "{text}");
        for other in [
            "com.apple.Notes",
            "com.apple.Safari",
            "com.apple.finder",
            "ai.qrtz.desktop",
        ] {
            assert!(!hidden(&fake, other), "{text}: {other} untouched");
        }
        assert!(w
            .apps
            .iter()
            .filter(|a| a.bundle_id != "com.google.Chrome")
            .all(|a| a.running));
        assert_eq!(
            w.frontmost_pid, before.frontmost_pid,
            "{text}: the user's app stays frontmost"
        );
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);

        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        assert!(raw.contains("intent.hide_app"), "{text}: picked at the intent");
        assert!(raw.contains("route.workspace.hide_app"));
        assert!(!raw.contains("cmd+h"), "{text}: no key event planned");
        assert!(
            !raw.contains("\"slack\""),
            "{text}: the task's words stay out of the log"
        );
    }
}

#[test]
fn the_app_the_user_is_in_is_never_hidden() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.frontmost_pid = 500;
    for a in w.apps.iter_mut() {
        a.hidden = false;
    }
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "hide slack");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert_eq!(v["state"]["held"], "no-route");
    let w = fake.world();
    assert!(w.acted.is_empty(), "nothing sent: {:?}", w.acted);
    assert!(!hidden(&fake, SLACK));
    assert_eq!(w.frontmost_pid, 500);
}

#[test]
fn hidden_already_or_not_running_is_done_with_nothing_acted() {
    // TextEdit is hidden already in this world; Chrome isn't running.
    for text in ["hide textedit", "hide chrome"] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.hide_app",
            "{text}: {v}"
        );
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        let w = fake.world();
        assert!(w.acted.is_empty(), "{text}: {:?}", w.acted);
        assert!(
            !w.apps
                .iter()
                .any(|a| a.bundle_id == "com.google.Chrome" && a.running),
            "{text}: nothing launched to hide it"
        );
    }
}

#[test]
fn a_helper_without_ctrl_apps_asks_out_and_other_phrasings_keep_their_intents() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.watch_since_ms = None;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "hide slack");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert_eq!(v["state"]["held"], "no-route");
    assert!(fake.world().acted.is_empty());
    for (text, intent) in [
        ("hide other apps", "intent.hide_others"),
        ("open slack", "intent.open_app"),
        ("quit slack", "intent.quit_app"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["decisions"][0]["chosen"], intent, "{text}: {v}");
    }
}

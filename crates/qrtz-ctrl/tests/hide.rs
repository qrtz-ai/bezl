//! ADR 0134: "hide other apps" / "hide all my apps" end to end on Linux, through the `ctrl:*` channels and the real
//! ctrl helper protocol to the `FakeHelper`. The intent layers pick it (no question at the intent), every running app
//! but the frontmost one and qrtz is hidden in one `NSRunningApplication.hide` route with no gate (it's reversible),
//! the Finder is hidden like the system's Hide Others does, the result is verified by the host from `ctrl.apps`, the
//! user's frontmost app / cursor / focus are untouched, and the log replays with no task words or app names.
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

#[test]
fn hide_other_apps_hides_everything_but_the_frontmost_and_qrtz_without_a_gate() {
    for text in [
        "hide other apps",
        "hide all my apps",
        "hide everything else",
        "every morning at 9 hide all my apps",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
        let w = fake.world();
        assert_eq!(w.acted.len(), 1, "{text}: one action");
        assert_eq!(
            w.acted[0].1,
            Route::Workspace(WorkspaceOp::HideApps {
                apps: vec![
                    "com.apple.Safari".into(),
                    "com.apple.finder".into(),
                    "com.tinyspeck.slackmacgap".into()
                ]
            }),
            "{text}"
        );
        for h in [
            "com.apple.Safari",
            "com.apple.finder",
            "com.tinyspeck.slackmacgap",
            "com.apple.TextEdit",
        ] {
            assert!(hidden(&fake, h), "{text}: {h} hidden");
        }
        assert!(
            !hidden(&fake, "com.apple.Notes"),
            "{text}: the frontmost app stays"
        );
        assert!(!hidden(&fake, "ai.qrtz.desktop"), "{text}: qrtz stays");
        // Nothing quit, and the user's frontmost app, cursor and focus are untouched.
        assert!(w
            .apps
            .iter()
            .filter(|a| a.bundle_id != "com.google.Chrome")
            .all(|a| a.running));
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);

        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        assert!(raw.contains("intent.hide_others"), "{text}: picked at the intent");
        assert!(raw.contains("route.workspace.hide_others"));
        for word in ["other apps", "all my apps", "everything else", "\"slack\""] {
            assert!(!raw.contains(word), "{text}: {word} reached the log");
        }
    }
}

#[test]
fn nothing_to_hide_is_done_and_a_helper_without_ctrl_apps_asks_out() {
    // Everything else already hidden: done, no action.
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    for a in w.apps.iter_mut().filter(|a| a.bundle_id != "com.apple.Notes") {
        a.hidden = true;
    }
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "hide other apps");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty());

    // No ctrl.apps (an older or non-mac helper): asked-out with no-route, nothing hidden.
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.watch_since_ms = None;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "hide all my apps");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert_eq!(v["state"]["held"], "no-route");
    assert!(fake.world().acted.is_empty());
}

#[test]
fn hiding_one_named_app_is_not_this_intent_and_quit_unused_still_wins_its_phrasing() {
    let d = tempfile::tempdir().unwrap();
    let (s, _) = rig(d.path(), world());
    let t = create(&s, "quit unused apps");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.quit_unused", "{v}");
    // "hide slack" names one app: never "hide everything else".
    for text in ["hide slack", "hide safari"] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(
            v["state"]["decisions"][0]["chosen"], "intent.hide_others",
            "{text}: {v}"
        );
        assert!(
            !fake
                .world()
                .acted
                .iter()
                .any(|a| matches!(a.1, Route::Workspace(WorkspaceOp::HideApps { .. }))),
            "{text}"
        );
    }
    let d = tempfile::tempdir().unwrap();
    let (s, _) = rig(d.path(), world());
    let t = create(&s, "close all my tabs");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.close_tabs", "{v}");
}

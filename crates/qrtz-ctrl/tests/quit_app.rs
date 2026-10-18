//! ADR 0148: "quit <app>" for one named app goes through the workspace (`NSRunningApplication.terminate()`, the
//! Dock's Quit) after the ADR 0111 confirm gate: no key event, no activation, never the app the user is in.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{Route, WorkspaceOp};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::schedule::IanaZone;
use qrtz_ctrl::service::CtrlService;
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

fn textedit(docs: &[(&str, bool)]) -> FakeApp {
    let mut a = FakeApp::new("com.apple.TextEdit", "TextEdit", 400);
    a.docs = docs.iter().map(|(t, u)| ((*t).to_string(), *u)).collect();
    a.windows = docs.len() as i64;
    a
}

fn world(docs: &[(&str, bool)]) -> FakeWorld {
    let mut w = FakeWorld::typical(2);
    w.apps.push(textedit(docs));
    w
}

struct Rig {
    s: CtrlService,
    fake: Arc<FakeHelper>,
}

fn rig(dir: &std::path::Path, w: FakeWorld, wall: u64) -> Rig {
    let fake = Arc::new(FakeHelper::new(w));
    let clock = Arc::new(FakeClock {
        wall: AtomicU64::new(wall),
        mono: AtomicU64::new(0),
    });
    let client = Arc::new(HelperClient::new(Box::new(fake.clone())));
    let s = CtrlService::open_with_zone(
        dir,
        Box::new(ClockHandle(clock.clone())),
        Box::new(IanaZone(Los_Angeles)),
    )
    .with_helper(client);
    Rig { s, fake }
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

fn kinds(v: &Value) -> Vec<String> {
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect()
}

fn count(v: &Value, kind: &str) -> usize {
    kinds(v).iter().filter(|k| *k == kind).count()
}

fn textedit_running(f: &FakeHelper) -> bool {
    f.world()
        .apps
        .iter()
        .find(|a| a.bundle_id == "com.apple.TextEdit")
        .unwrap()
        .running
}

const TEXTEDIT: &str = "com.apple.TextEdit";

#[test]
fn quit_a_named_app_waits_at_the_gate_then_terminates_it_without_a_key_or_activation() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), world(&[("Notes.txt", false), ("Draft.txt", true)]), T0);
    let t = create(&r.s, "quit textedit");
    let v = run_now(&r.s, &t);
    let st = &v["state"];
    assert_eq!(st["status"], "needs-confirm", "{v}");
    assert_eq!(
        st["confirm"]["rules"],
        json!([
            "choose_intent:intent.quit_app",
            "choose_move:route.workspace.quit_app"
        ])
    );
    let unsaved: Vec<bool> = st["confirm"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["unsaved"].as_bool().unwrap())
        .collect();
    assert_eq!(
        unsaved,
        vec![false, true],
        "the card says which window has unsaved work"
    );
    assert!(r.fake.world().acted.is_empty(), "nothing before the answer");

    let v = confirm(&r.s, &v, "approve");
    assert_eq!(v["state"]["status"], "done", "{v}");
    let w = r.fake.world();
    let routes: Vec<&Route> = w.acted.iter().map(|(_, r)| r).collect();
    assert_eq!(
        routes,
        vec![&Route::Workspace(WorkspaceOp::QuitApps {
            apps: vec![TEXTEDIT.to_string()]
        })],
        "exactly one workspace quit of that app"
    );
    assert!(!textedit_running(&r.fake));
    assert_eq!(w.frontmost_pid, 100, "the user's app stays frontmost");
    assert_eq!(count(&v, "action.requested"), 1);
    assert!(!format!("{v}").contains("cmd+q"), "no key event planned or sent");
}

#[test]
fn deny_leaves_the_app_running() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), world(&[("Draft.txt", true)]), T0);
    let t = create(&r.s, "quit textedit");
    let v = run_now(&r.s, &t);
    let v = confirm(&r.s, &v, "deny");
    assert_eq!(v["state"]["status"], "cancelled", "{v}");
    assert!(textedit_running(&r.fake));
    assert!(r.fake.world().acted.is_empty());
}

#[test]
fn the_app_the_user_is_in_is_never_quit() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world(&[("Notes.txt", false)]);
    w.frontmost_pid = 400;
    let r = rig(d.path(), w, T0);
    let t = create(&r.s, "quit textedit");
    let v = run_now(&r.s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert!(
        v["state"]["confirm"].is_null(),
        "no card for a quit that can't happen"
    );
    assert_eq!(count(&v, "exec.held"), 1);
    assert_eq!(
        count(&v, "action.requested"),
        0,
        "no other intent tried in its place"
    );
    assert!(textedit_running(&r.fake), "the frontmost app keeps running");
    let w = r.fake.world();
    assert!(w.acted.is_empty(), "nothing sent: {:?}", w.acted);
    assert_eq!(w.frontmost_pid, 400);
}

#[test]
fn an_app_that_is_not_running_quits_nothing() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world(&[]);
    w.apps
        .iter_mut()
        .find(|a| a.bundle_id == TEXTEDIT)
        .unwrap()
        .running = false;
    let r = rig(d.path(), w, T0);
    let t = create(&r.s, "quit textedit");
    let mut v = run_now(&r.s, &t);
    if v["state"]["status"] == "needs-confirm" {
        v = confirm(&r.s, &v, "approve");
    }
    let w = r.fake.world();
    assert!(
        !w.acted
            .iter()
            .any(|(_, r)| matches!(r, Route::PidKey { .. } | Route::Activate { .. })),
        "{:?}",
        w.acted
    );
    assert!(!textedit_running(&r.fake), "still not running: {v}");
}

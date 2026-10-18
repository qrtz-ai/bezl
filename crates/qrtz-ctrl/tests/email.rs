//! ADR 0138: "open my email" / "check my inbox" end to end on Linux. "My email" is the Mac's Mail (as "my calendar"
//! is Calendar, ADR 0128) unless a mail app is named; a running Mail is already open (nothing launched, nothing
//! hidden, nothing brought forward); a Mail that isn't running asks (a cold launch comes to the front even with
//! `activates = false`, ADR 0100) and nothing launches; "send an email" is not this.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{Consent, Route, ScriptVerb, WorkspaceOp};
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

fn with_mail(running: bool) -> FakeWorld {
    let mut w = world();
    w.apps.push(FakeApp {
        running,
        ..FakeApp::new("com.apple.mail", "Mail", 500)
    });
    w
}

#[test]
fn a_running_mail_is_already_open_and_nothing_is_taken_over() {
    for text in [
        "open my email",
        "every morning open my email",
        "check my email",
        "open my inbox",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), with_mail(true));
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.open_app", "{text}");
        assert_eq!(
            v["state"]["decisions"][1]["chosen"], "app.com.apple.mail",
            "{text}"
        );
        let w = fake.world();
        assert_eq!(
            w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
            vec![Route::Workspace(WorkspaceOp::LaunchHidden {
                app: "com.apple.mail".into()
            })],
            "{text}"
        );
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        assert!(w.apps.iter().all(|a| !a.hidden), "{text}: nothing hidden");
        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        assert!(
            !raw.contains("email") && !raw.contains("inbox"),
            "{text}: task words in the log"
        );
    }
}

#[test]
fn a_mail_that_isnt_running_asks_instead_of_coming_to_the_front() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), with_mail(false));
    let t = create(&s, "every morning open my email");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert!(
        !w.apps
            .iter()
            .find(|a| a.bundle_id == "com.apple.mail")
            .unwrap()
            .running
    );
}

#[test]
fn send_an_email_and_other_tasks_are_not_this() {
    for text in ["send an email to sam", "mute my mac", "close all my tabs"] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), with_mail(true));
        let t = create(&s, text);
        let v = run_now(&s, &t);
        let opened_mail = fake.world().acted.iter().any(|a| {
            a.1 == Route::Workspace(WorkspaceOp::LaunchHidden {
                app: "com.apple.mail".into(),
            })
        });
        assert!(!opened_mail, "{text}: {v}");
    }
    let _ = (Consent::Granted, ScriptVerb::CountTabs);
}

//! ADR 0128: "every morning open my calendar". "my calendar" is the Mac's Calendar (com.apple.iCal), named in full
//! over a calendar app it names in part; an already running Calendar counts as open (nothing launched, nothing
//! hidden); a Calendar that isn't running asks (a cold launch would come to the front, ADR 0100); a named calendar
//! site opens in the running default browser in the background.
use qrtz_ctrl::decide::{replay_check, run_chain, Goal, Heuristic, Outcome};
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{Route, WorkspaceOp};
use qrtz_ctrl::layers::{app_entry, label, AppCatalog, AppsObs};
use qrtz_ctrl::log::{Event, EventLog};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::service::CtrlService;
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

struct Tick(AtomicU64);
impl Clock for Tick {
    fn mono_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) * 1000
    }
    fn wall_ms(&self) -> u64 {
        1_790_000_000_000
    }
}

fn catalog(names: &[&str]) -> AppsObs {
    AppCatalog {
        apps: names.iter().map(|n| app_entry(n)).collect(),
        default_browser: None,
        ax: false,
    }
    .observe()
}

fn chain(text: &str, apps: AppsObs) -> Vec<(String, Outcome)> {
    chain_labels(text, apps)
        .into_iter()
        .map(|(c, o, _)| (c, o))
        .collect()
}

/// The chosen option's label (UI copy, resolved from the goal) at decision `i`.
fn label_of(text: &str, i: usize) -> String {
    chain_labels(text, catalog(MAC)).remove(i).2
}

fn chain_labels(text: &str, apps: AppsObs) -> Vec<(String, Outcome, String)> {
    let mut log = EventLog::new("run-1");
    for e in [
        Event::TaskStarted {
            task_id: "task-0".into(),
        },
        Event::IntentPending {
            task_id: "task-0".into(),
        },
        Event::ObsApps { apps },
    ] {
        log.append(0, 0, e);
    }
    let goal = Goal {
        task_id: "task-0",
        text,
    };
    let ev = run_chain(&goal, &log.state(), &Heuristic, &Tick(AtomicU64::new(0)));
    for e in &ev {
        log.append(0, 0, e.clone());
    }
    replay_check(&log, &goal).unwrap();
    ev.into_iter()
        .filter_map(|e| match e {
            Event::DecisionMade {
                chosen,
                outcome,
                options,
                ..
            } => {
                let l = options
                    .iter()
                    .find(|o| o.id == chosen)
                    .map(|o| label(o, Some(&goal)))
                    .unwrap_or_default();
                Some((chosen, outcome, l))
            }
            _ => None,
        })
        .collect()
}

const MAC: &[&str] = &[
    "Safari",
    "Google Chrome",
    "Finder",
    "System Settings",
    "Messages",
    "Notes",
    "Mail",
    "Calendar",
    "Clock",
];

#[test]
fn my_calendar_is_the_macs_calendar_app_started_hidden() {
    for text in [
        "every morning open my calendar",
        "open my calendar every morning",
        "open calendar",
        "show me my calendar",
    ] {
        let d = chain(text, catalog(MAC));
        let ids: Vec<&str> = d.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(
            ids,
            [
                "intent.open_app",
                "app.com.apple.iCal",
                "route.workspace.launch_hidden"
            ],
            "{text}"
        );
        assert!(d.iter().all(|(_, o)| *o == Outcome::Act), "{text}: {d:?}");
    }
    // Another calendar app named in part loses to the one named in full; named in full, it wins.
    let mut with_notion = MAC.to_vec();
    with_notion.push("Notion Calendar");
    let d = chain("every morning open my calendar", catalog(&with_notion));
    assert_eq!(d[1].0, "app.com.apple.iCal");
    assert_ne!(d[1].1, Outcome::Ask);
    let d = chain("every morning open notion calendar", catalog(&with_notion));
    assert_eq!(d[0].0, "intent.open_app");
    assert_eq!(d[1].0, "app.name.notion-calendar");
    assert_ne!(d[1].1, Outcome::Ask);
}

#[test]
fn a_calendar_site_is_a_site_not_the_app() {
    for (text, want) in [
        ("every morning open my google calendar", "calendar.google.com"),
        ("open gcal", "calendar.google.com"),
        ("open my outlook calendar", "outlook.office.com/calendar"),
        ("every morning open calendar.google.com", "calendar.google.com"),
    ] {
        let d = chain(text, catalog(MAC));
        let ids: Vec<&str> = d.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(ids[0], "intent.open_site", "{text}");
        assert_eq!(ids[3], "route.workspace.open_url", "{text}");
        assert!(d.iter().all(|(_, o)| *o == Outcome::Act), "{text}: {d:?}");
        assert!(label_of(text, 2).contains(want), "{text}");
    }
}

fn svc(dir: &std::path::Path, world: FakeWorld) -> (CtrlService, Arc<FakeHelper>) {
    let fake = Arc::new(FakeHelper::new(world));
    let client = Arc::new(HelperClient::new(Box::new(fake.clone())));
    let s = CtrlService::open(dir, Box::new(Tick(AtomicU64::new(0)))).with_helper(client);
    (s, fake)
}

fn run(s: &CtrlService, text: &str) -> Value {
    let id = s
        .call(
            "ctrl:create-task",
            json!({"text": text, "schedule": {"kind": "daily", "hour": 8, "minute": 0}}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = s.call("ctrl:run-now", json!({"id": id})).unwrap();
    s.call("ctrl:run", json!({"runId": r["id"]})).unwrap()
}

fn untouched(w: &FakeWorld) {
    let t = FakeWorld::typical(0);
    assert_eq!(w.frontmost_pid, t.frontmost_pid, "frontmost app changed");
    assert_eq!(w.cursor, t.cursor, "cursor moved");
    assert_eq!(w.focused, t.focused, "focus changed");
}

fn persisted(dir: &std::path::Path, v: &Value, text: &str) {
    let store = Store::open(dir).unwrap();
    let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
    log.verify().unwrap();
    let task_id = v["run"]["taskId"].as_str().unwrap();
    replay_check(&log, &Goal { task_id, text }).unwrap();
    let raw = log.to_jsonl().to_lowercase();
    for w in ["morning", "my calendar", "google calendar"] {
        assert!(!raw.contains(w), "the log holds {w:?}");
    }
}

fn world(calendar_running: bool) -> FakeWorld {
    let mut w = FakeWorld::typical(2);
    w.apps.push(FakeApp {
        running: calendar_running,
        ..FakeApp::new("com.apple.iCal", "Calendar", 400)
    });
    w
}

#[test]
fn a_running_calendar_is_already_open_and_nothing_is_taken_over() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = svc(d.path(), world(true));
    let text = "every morning open my calendar";
    let v = run(&s, text);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(
        v["state"]["verified"],
        json!([{"actionId": "a0", "outcome": "passed"}])
    );
    let w = fake.world();
    assert_eq!(
        w.acted,
        vec![(
            "a0".to_string(),
            Route::Workspace(WorkspaceOp::LaunchHidden {
                app: "com.apple.iCal".into()
            })
        )]
    );
    assert!(w.opened_urls.is_empty());
    untouched(&w);
    assert_eq!(v["state"]["violations"], 0);
    persisted(d.path(), &v, text);
}

#[test]
fn a_calendar_that_isnt_running_asks_instead_of_coming_to_the_front() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = svc(d.path(), world(false));
    let text = "every morning open my calendar";
    let v = run(&s, text);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert!(
        !w.apps
            .iter()
            .find(|a| a.bundle_id == "com.apple.iCal")
            .unwrap()
            .running
    );
    untouched(&w);
    persisted(d.path(), &v, text);
}

#[test]
fn google_calendar_opens_in_the_running_default_browser_in_the_background() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = svc(d.path(), world(false));
    let text = "every morning open my google calendar";
    let v = run(&s, text);
    assert_eq!(v["state"]["status"], "done", "{v}");
    let w = fake.world();
    assert_eq!(w.opened_urls, vec!["https://calendar.google.com".to_string()]);
    assert!(
        !w.apps
            .iter()
            .find(|a| a.bundle_id == "com.apple.iCal")
            .unwrap()
            .running
    );
    untouched(&w);
    persisted(d.path(), &v, text);
}

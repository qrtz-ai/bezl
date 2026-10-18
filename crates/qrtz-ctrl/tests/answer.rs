//! Slice 4 (ADR 0106) through the `ctrl:*` channels: a run decides on run-now, an unclear task asks, `ctrl:answer`
//! resumes the chain (or cancels the run), and the persisted run replays to the same questions after a restart.
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::layers::{app_entry, AppCatalog, FixedApps};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::service::{CtrlErrorCode, CtrlService};
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

const T0: u64 = 1_790_000_000_000;

struct Tick(AtomicU64);
impl Clock for Tick {
    fn mono_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) * 1000
    }
    fn wall_ms(&self) -> u64 {
        T0
    }
}

fn svc(dir: &std::path::Path) -> CtrlService {
    let apps = AppCatalog {
        apps: ["Safari", "Google Chrome", "Finder"]
            .iter()
            .map(|n| app_entry(n))
            .collect(),
        default_browser: Some("com.apple.Safari".into()),
        ax: false,
    };
    CtrlService::open(dir, Box::new(Tick(AtomicU64::new(0)))).with_apps(Box::new(FixedApps(apps)))
}

fn task(s: &CtrlService, text: &str) -> String {
    s.call(
        "ctrl:create-task",
        json!({"text": text, "schedule": {"kind": "once", "at": null}}),
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_clear_task_plans_and_the_view_shows_the_decisions() {
    let d = tempfile::tempdir().unwrap();
    let s = svc(d.path());
    let id = task(&s, "every day close all my tabs");
    let run = s.call("ctrl:run-now", json!({"id": id})).unwrap();
    assert_eq!(run["status"], "planned", "{run}");
    let v = s.call("ctrl:run", json!({"runId": run["id"]})).unwrap();
    let st = &v["state"];
    assert_eq!(st["ask"], Value::Null);
    let steps: Vec<&str> = st["plan"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        steps,
        [
            "close browser tabs",
            "safari",
            "all tabs",
            "close every tab by script"
        ]
    );
    assert_eq!(st["plan"]["irreversible"], false);
    let app = &st["decisions"][1];
    assert_eq!(app["node"], "choose_app");
    assert_eq!(app["options"][0]["label"], "safari", "the default browser first");
    assert_eq!(app["outcome"], "act");
    assert_eq!(app["model"], "qrtz-heuristic 1");
    assert!(app["p"].as_f64().unwrap() >= 0.8);
    // Scrubbing back before the decisions shows none of them.
    let early = s.call("ctrl:run", json!({"runId": run["id"], "at": 3})).unwrap();
    assert_eq!(early["state"]["decisions"], json!([]));
    assert_eq!(early["state"]["plan"], Value::Null);
}

#[test]
fn an_unclear_task_asks_and_the_answer_resumes_the_chain_and_survives_a_restart() {
    let d = tempfile::tempdir().unwrap();
    let s = svc(d.path());
    let id = task(&s, "do the thing");
    let run = s.call("ctrl:run-now", json!({"id": id})).unwrap();
    assert_eq!(run["status"], "asking", "{run}");
    let run_id = run["id"].as_str().unwrap().to_string();
    let v = s.call("ctrl:run", json!({"runId": run_id})).unwrap();
    let ask = &v["state"]["ask"];
    assert_eq!(ask["node"], "choose_intent");
    assert_eq!(ask["options"].as_array().unwrap().len(), 3);
    assert_eq!(ask["options"][1]["label"], "close browser tabs");
    let decision = ask["decisionId"].as_str().unwrap().to_string();

    let bad = s
        .call(
            "ctrl:answer",
            json!({"runId": run_id, "decisionId": decision, "optionId": "none"}),
        )
        .unwrap_err();
    assert_eq!(bad.code, CtrlErrorCode::Validation);
    let answer = |decision: &str, option: &str| {
        s.call(
            "ctrl:answer",
            json!({"runId": run_id, "decisionId": decision, "optionId": option}),
        )
        .unwrap()
    };
    // Close tabs → the default browser acts → which tabs? is unclear too: a second ask.
    let mid = answer(&decision, "intent.close_tabs");
    assert_eq!(mid["run"]["status"], "asking", "{mid}");
    assert_eq!(mid["state"]["ask"]["node"], "choose_target");
    let second = mid["state"]["ask"]["decisionId"].as_str().unwrap().to_string();
    let after = answer(&second, "scope.all_tabs");
    assert_eq!(after["run"]["status"], "planned", "{after}");
    let kinds: Vec<&str> = after["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == "decision.answered_by_user")
            .count(),
        2
    );
    assert_eq!(kinds.last(), Some(&"plan.produced"));
    assert_eq!(after["state"]["decisions"][0]["answered"], true);
    let steps: Vec<&str> = after["state"]["plan"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        steps,
        [
            "close browser tabs",
            "safari",
            "all tabs",
            "close every tab by script"
        ]
    );
    let pick = "intent.close_tabs";
    let again = s
        .call(
            "ctrl:answer",
            json!({"runId": run_id, "decisionId": decision, "optionId": pick}),
        )
        .unwrap_err();
    assert_eq!(again.code, CtrlErrorCode::Validation);

    // Restart: the persisted run verifies, folds to the same state and replays to the same questions.
    drop(s);
    let s2 = svc(d.path());
    assert_eq!(s2.call("ctrl:run", json!({"runId": run_id})).unwrap(), after);
    let store = Store::open(d.path()).unwrap();
    let log = store.run(&run_id).unwrap();
    let text = store.task(&id).unwrap().text.clone();
    replay_check(
        &log,
        &Goal {
            task_id: &id,
            text: &text,
        },
    )
    .unwrap();
}

#[test]
fn cancelling_an_ask_finishes_the_run() {
    let d = tempfile::tempdir().unwrap();
    let s = svc(d.path());
    let id = task(&s, "hmm");
    let run = s.call("ctrl:run-now", json!({"id": id})).unwrap();
    let v = s.call("ctrl:run", json!({"runId": run["id"]})).unwrap();
    let decision = v["state"]["ask"]["decisionId"].clone();
    let done = s
        .call(
            "ctrl:answer",
            json!({"runId": run["id"], "decisionId": decision, "optionId": null}),
        )
        .unwrap();
    assert_eq!(done["run"]["status"], "cancelled");
    assert_eq!(done["state"]["ask"], Value::Null);
}

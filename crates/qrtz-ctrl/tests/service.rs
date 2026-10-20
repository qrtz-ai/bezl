//! The `ctrl:*` channels over a fake helper and a temp store: `ctrl:do`, asks, confirms, stop, history, and what
//! reaches the disk (the command in `do.jsonl`; never tree text or the goal in a run file).
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::service::{CtrlErrorCode, CtrlService};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

struct Tick(AtomicU64);
impl Clock for Tick {
    fn mono_ns(&self) -> u64 {
        self.0.fetch_add(1_000_000, Ordering::Relaxed)
    }
    fn wall_ms(&self) -> u64 {
        1_800_000_000_000
    }
}

fn app(name: &str) -> FakeApp {
    let p = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn service(dir: &std::path::Path) -> (CtrlService, Arc<FakeHelper>) {
    let apps: Vec<FakeApp> = ["safari", "notes", "finder", "settings", "mail", "slack"]
        .into_iter()
        .map(app)
        .collect();
    let mut w = FakeWorld {
        default_browser: Some("com.apple.Safari".into()),
        frontmost: Some("com.apple.finder".into()),
        ..Default::default()
    };
    for a in &apps {
        if a.running {
            w.state.insert(a.bundle_id.clone(), a.start.clone());
        }
    }
    w.apps = apps;
    let fake = Arc::new(FakeHelper::new(w));
    let svc = CtrlService::open(dir, Box::new(Tick(AtomicU64::new(0))))
        .with_scorer(Box::new(TestModel))
        .with_helper(Arc::new(HelperClient::new(Box::new(fake.clone()))));
    (svc, fake)
}

#[test]
fn do_runs_the_chain_and_the_history_shows_it() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _) = service(dir.path());
    let v = svc
        .call("ctrl:do", json!({ "text": "  open   youtube " }))
        .unwrap();
    assert_eq!(v["status"], "done", "{v:#}");
    assert_eq!(v["text"], "open youtube");
    assert_eq!(v["app"], "safari");
    let labels: Vec<&str> = v["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        vec![
            "open safari",
            "press cmd+l",
            "type “youtube.com” and press return"
        ]
    );
    let runs = svc.call("ctrl:runs", json!({})).unwrap();
    assert_eq!(runs.as_array().unwrap().len(), 1);
    let run_id = v["runId"].as_str().unwrap();
    let early = svc.call("ctrl:run", json!({ "runId": run_id, "at": 2 })).unwrap();
    assert_eq!(early["status"], "running");
    assert!(early["actions"].as_array().unwrap().is_empty());

    // On disk: the command is in do.jsonl (Jared's own text); run files hold no goal text and no tree text.
    let runs_dir = dir.path().join("do").join("runs");
    for f in std::fs::read_dir(&runs_dir).unwrap() {
        let body = std::fs::read_to_string(f.unwrap().path()).unwrap().to_lowercase();
        for bad in ["youtube", "address and search", "start page", "favorites"] {
            assert!(!body.contains(bad), "{bad} in a run file");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(dir.path().join("do"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(m & 0o777, 0o700);
    }
}

#[test]
fn a_confirm_waits_and_approve_acts() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, fake) = service(dir.path());
    let v = svc
        .call("ctrl:do", json!({ "text": "press send in mail" }))
        .unwrap();
    assert_eq!(v["status"], "needs-confirm", "{v:#}");
    assert_eq!(v["confirm"]["rules"], json!(["label:send"]));
    assert_eq!(svc.take_confirm_notices().len(), 1);
    assert!(fake.world().acts.iter().all(|a| a["action"] != "press"));
    let run = v["runId"].as_str().unwrap();
    let gate = v["confirm"]["gateId"].as_str().unwrap();
    let v = svc
        .call(
            "ctrl:confirm",
            json!({ "runId": run, "gateId": gate, "answer": "approve" }),
        )
        .unwrap();
    assert_eq!(v["status"], "done", "{v:#}");
    let again = svc.call(
        "ctrl:confirm",
        json!({ "runId": run, "gateId": gate, "answer": "approve" }),
    );
    assert_eq!(again.unwrap_err().code, CtrlErrorCode::Validation);
}

#[test]
fn deny_cancels_and_an_ask_can_be_answered_or_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _) = service(dir.path());
    let v = svc
        .call("ctrl:do", json!({ "text": "press send in mail" }))
        .unwrap();
    let (run, gate) = (
        v["runId"].as_str().unwrap(),
        v["confirm"]["gateId"].as_str().unwrap(),
    );
    let v = svc
        .call(
            "ctrl:confirm",
            json!({ "runId": run, "gateId": gate, "answer": "deny" }),
        )
        .unwrap();
    assert_eq!(v["status"], "cancelled");

    let v = svc
        .call(
            "ctrl:do",
            json!({ "text": "message the design channel in slack" }),
        )
        .unwrap();
    assert_eq!(v["status"], "asking", "{v:#}");
    let opts = v["ask"]["options"].as_array().unwrap();
    assert!(!opts.is_empty() && opts.len() <= 3);
    assert!(opts
        .iter()
        .all(|o| o["label"].as_str().unwrap() == o["label"].as_str().unwrap().to_lowercase()));
    let run = v["runId"].as_str().unwrap().to_string();
    let bad = svc.call(
        "ctrl:answer",
        json!({ "runId": run, "decisionId": v["ask"]["decisionId"], "optionId": "app.evil" }),
    );
    assert_eq!(bad.unwrap_err().code, CtrlErrorCode::Validation);
    let v = svc
        .call(
            "ctrl:answer",
            json!({ "runId": run, "decisionId": v["ask"]["decisionId"], "optionId": Value::Null }),
        )
        .unwrap();
    assert_eq!(v["status"], "cancelled");
}

#[test]
fn a_new_command_cancels_a_waiting_run_and_stop_ends_one() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _) = service(dir.path());
    let first = svc
        .call("ctrl:do", json!({ "text": "press send in mail" }))
        .unwrap();
    let run1 = first["runId"].as_str().unwrap().to_string();
    let second = svc
        .call("ctrl:do", json!({ "text": "press send in mail" }))
        .unwrap();
    let v = svc.call("ctrl:run", json!({ "runId": run1 })).unwrap();
    assert_eq!(v["status"], "cancelled");
    let v = svc
        .call("ctrl:stop", json!({ "runId": second["runId"] }))
        .unwrap();
    assert_eq!(v, json!({ "runId": second["runId"], "status": "stopped" }));
    let v = svc.call("ctrl:run", json!({ "runId": second["runId"] })).unwrap();
    assert_eq!(v["stop"], "stop-button");
}

#[test]
fn without_a_helper_do_is_unavailable_and_bad_input_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let svc = CtrlService::open(dir.path(), Box::new(Tick(AtomicU64::new(0))));
    assert_eq!(
        svc.call("ctrl:do", json!({ "text": "open youtube" }))
            .unwrap_err()
            .code,
        CtrlErrorCode::Unavailable
    );
    assert_eq!(svc.call("ctrl:runs", json!({})).unwrap(), json!([]));
    let (svc, _) = service(dir.path());
    assert_eq!(
        svc.call("ctrl:do", json!({ "text": "   " })).unwrap_err().code,
        CtrlErrorCode::Validation
    );
    assert_eq!(
        svc.call("ctrl:run", json!({ "runId": "../x" })).unwrap_err().code,
        CtrlErrorCode::NotFound
    );
    assert_eq!(
        svc.call("ctrl:tasks", json!({})).unwrap_err().code,
        CtrlErrorCode::Unavailable
    );
}

/// A test stand-in for ctrl's decision model (QRT-293: ctrl ships no rules scorer and refuses without a ready model):
/// the option sources' ranks as its logits. Lives only in this test.
struct TestModel;
impl qrtz_ctrl::decide::Scorer for TestModel {
    fn model(&self) -> qrtz_ctrl::decide::ModelId {
        qrtz_ctrl::decide::ModelId {
            id: "test-model".into(),
            version: "1".into(),
            quant: None,
        }
    }
    fn logits(
        &self,
        q: &qrtz_ctrl::decide::Question,
        _g: &qrtz_ctrl::decide::Goal,
    ) -> Result<Vec<i64>, String> {
        Ok(q.choices.iter().map(|c| c.prior).collect())
    }
    fn status(&self) -> qrtz_ctrl::decide::ModelStatus {
        qrtz_ctrl::decide::ModelStatus::Ready
    }
}

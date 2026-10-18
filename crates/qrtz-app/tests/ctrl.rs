//! ctrl through the real router (ADR 0103): the generated contract validates requests and responses, the ctrl
//! manifest's capabilities gate the channels, and the store under the data dir survives a restart.
use qrtz_app::ports::*;
use qrtz_app::*;
use qrtz_ctrl::service::{CtrlService, SystemClock};
use qrtz_ipc::Router;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

fn app(dir: &Path, with_ctrl: bool) -> Router<Shared> {
    let svc = AppService::new(AppDeps {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "tauri test".into(),
        },
        config: ConfigStore::open(dir.join("config.json")).into(),
        runtime: Arc::new(StubRuntime::default()),
        storage: Arc::new(StubStorage),
        talk: Arc::new(StubTalk),
        shell: Arc::new(NoShell),
        helper: None,
        events: Events::new(),
    });
    if with_ctrl {
        svc.set_ctrl(Arc::new(CtrlService::open(
            dir.join("ctrl"),
            Box::new(SystemClock::default()),
        )));
    }
    Router::new(Shared(svc))
}

async fn call(r: &Router<Shared>, sub: &str, ch: &str, payload: Value) -> Value {
    r.dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":sub,"payload":payload}))
        .await
}

#[tokio::test]
async fn create_list_run_and_scrub_through_the_router_and_a_restart() {
    let d = tempfile::tempdir().unwrap();
    let r = app(d.path(), true);
    assert_eq!(
        call(&r, "ctrl", "ctrl:tasks", Value::Null).await["data"],
        json!([])
    );
    let t = call(
        &r,
        "ctrl",
        "ctrl:create-task",
        json!({"text": "every day close all my tabs", "schedule": {"kind": "daily", "hour": 9, "minute": 0}}),
    )
    .await;
    assert_eq!(t["ok"], true, "{t}");
    assert!(
        t["data"]["nextRunAt"].as_u64().is_some(),
        "daily has a next due: {t}"
    );
    let id = t["data"]["id"].as_str().unwrap().to_string();
    let run = call(&r, "ctrl", "ctrl:run-now", json!({"id": id})).await;
    // The decision layers (ADR 0106) plan it right away; nothing executes (slice 5).
    assert_eq!(run["data"]["status"], "planned", "{run}");
    let run_id = run["data"]["id"].as_str().unwrap().to_string();

    // A new app over the same data dir (a relaunch).
    let r = app(d.path(), true);
    let tasks = call(&r, "ctrl", "ctrl:tasks", Value::Null).await;
    assert_eq!(tasks["data"][0]["text"], "every day close all my tabs");
    assert_eq!(tasks["data"][0]["runs"], 1);
    let runs = call(&r, "ctrl", "ctrl:runs", json!({"taskId": id})).await;
    assert_eq!(runs["data"][0]["id"], run_id.as_str());
    let at1 = call(&r, "ctrl", "ctrl:run", json!({"runId": run_id, "at": 1})).await;
    assert_eq!(at1["data"]["state"]["status"], "running", "{at1}");
    assert_eq!(at1["data"]["events"][1]["kind"], "intent.pending");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&d.path().join("ctrl")), 0o700);
        assert_eq!(mode(&d.path().join("ctrl/tasks.jsonl")), 0o600);
    }
}

#[tokio::test]
async fn an_unclear_task_asks_and_answers_resume_the_decision_layers_through_the_router() {
    let d = tempfile::tempdir().unwrap();
    let r = app(d.path(), true);
    let t = call(
        &r,
        "ctrl",
        "ctrl:create-task",
        json!({"text": "do the thing", "schedule": {"kind": "once", "at": null}}),
    )
    .await;
    let run = call(&r, "ctrl", "ctrl:run-now", json!({"id": t["data"]["id"]})).await;
    assert_eq!(run["data"]["status"], "asking", "{run}");
    let run_id = run["data"]["id"].clone();
    let v = call(&r, "ctrl", "ctrl:run", json!({"runId": run_id})).await;
    assert_eq!(v["ok"], true, "the response matches the contract: {v}");
    let ask = &v["data"]["state"]["ask"];
    assert_eq!(ask["node"], "choose_intent");
    let a1 = call(
        &r,
        "ctrl",
        "ctrl:answer",
        json!({"runId": run_id, "decisionId": ask["decisionId"], "optionId": "intent.close_tabs"}),
    )
    .await;
    assert_eq!(a1["data"]["run"]["status"], "asking", "{a1}");
    let ask2 = a1["data"]["state"]["ask"].clone();
    assert_eq!(ask2["node"], "choose_target");
    let a2 = call(
        &r,
        "ctrl",
        "ctrl:answer",
        json!({"runId": run_id, "decisionId": ask2["decisionId"], "optionId": "scope.all_tabs"}),
    )
    .await;
    assert_eq!(a2["data"]["run"]["status"], "planned", "{a2}");
    assert_eq!(
        a2["data"]["state"]["plan"]["steps"][1]["label"],
        "your default browser"
    );
    // Answering again, or a run id that isn't one, is refused; other sub apps can't answer.
    let again = call(
        &r,
        "ctrl",
        "ctrl:answer",
        json!({"runId": run_id, "decisionId": ask2["decisionId"], "optionId": "scope.all_tabs"}),
    )
    .await;
    assert_eq!(again["error"]["code"], "E_VALIDATION", "{again}");
    let other = call(
        &r,
        "talk",
        "ctrl:answer",
        json!({"runId": run_id, "decisionId": "d0", "optionId": null}),
    )
    .await;
    assert_eq!(other["error"]["code"], "E_FORBIDDEN");
}

#[tokio::test]
async fn ctrl_channels_are_validated_gated_and_honest_when_missing() {
    let d = tempfile::tempdir().unwrap();
    let r = app(d.path(), true);
    let bad = call(
        &r,
        "ctrl",
        "ctrl:create-task",
        json!({"text": "x", "schedule": {"kind": "daily", "hour": 24, "minute": 0}}),
    )
    .await;
    assert_eq!(bad["error"]["code"], "E_VALIDATION", "{bad}");
    let empty = call(
        &r,
        "ctrl",
        "ctrl:create-task",
        json!({"text": "  ", "schedule": {"kind": "once", "at": null}}),
    )
    .await;
    assert_eq!(empty["error"]["code"], "E_VALIDATION", "{empty}");
    // Other sub apps don't hold ctrl's capabilities.
    assert_eq!(
        call(&r, "talk", "ctrl:tasks", Value::Null).await["error"]["code"],
        "E_FORBIDDEN"
    );
    assert_eq!(
        call(&r, "ctrl", "ctrl:run-now", json!({"id": "task-9"})).await["error"]["code"],
        "E_NOT_FOUND"
    );
    let none = app(d.path(), false);
    let v = call(&none, "ctrl", "ctrl:tasks", Value::Null).await;
    assert_eq!(v["error"]["code"], "E_UNAVAILABLE", "{v}");
}

/// Slice 5 (ADR 0108) through the router: with the ctrl helper connected (the in-memory fake speaking the real
/// ctrl protocol), "close all my tabs" executes in the background, verifies, and every response validates against
/// the generated contract; an irreversible task stops at needs-confirm with nothing acted.
#[tokio::test]
async fn with_a_helper_a_run_executes_verifies_and_holds_irreversible_plans_through_the_router() {
    use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
    let d = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeHelper::new(FakeWorld::typical(4)));
    let svc = AppService::new(AppDeps {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "tauri test".into(),
        },
        config: ConfigStore::open(d.path().join("config.json")).into(),
        runtime: Arc::new(StubRuntime::default()),
        storage: Arc::new(StubStorage),
        talk: Arc::new(StubTalk),
        shell: Arc::new(NoShell),
        helper: None,
        events: Events::new(),
    });
    svc.set_ctrl(Arc::new(
        CtrlService::open(d.path().join("ctrl"), Box::new(SystemClock::default()))
            .with_helper(Arc::new(HelperClient::new(Box::new(fake.clone())))),
    ));
    let r = Router::new(Shared(svc));
    for (text, status) in [
        ("every day close all my tabs", "done"),
        ("every week empty the trash", "needs-confirm"),
    ] {
        let t = call(
            &r,
            "ctrl",
            "ctrl:create-task",
            json!({"text": text, "schedule": {"kind": "once", "at": null}}),
        )
        .await;
        let id = t["data"]["id"].as_str().unwrap().to_string();
        let run = call(&r, "ctrl", "ctrl:run-now", json!({"id": id})).await;
        assert_eq!(run["ok"], true, "{run}");
        assert_eq!(run["data"]["status"], status, "{run}");
        let v = call(&r, "ctrl", "ctrl:run", json!({"runId": run["data"]["id"]})).await;
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["data"]["state"]["violations"], 0);
    }
    let w = fake.world();
    assert_eq!(w.acted.len(), 1, "only the reversible task acted");
    assert_eq!(w.frontmost_pid, 100, "the user's app stayed frontmost");
    assert_eq!(
        w.apps
            .iter()
            .find(|a| a.bundle_id == "com.apple.Safari")
            .unwrap()
            .tabs,
        0
    );
}

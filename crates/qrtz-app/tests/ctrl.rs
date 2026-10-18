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
    let id = t["data"]["id"].as_str().unwrap().to_string();
    let run = call(&r, "ctrl", "ctrl:run-now", json!({"id": id})).await;
    assert_eq!(run["data"]["status"], "pending-intent", "{run}");
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

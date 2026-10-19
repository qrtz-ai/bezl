//! ctrl do through the real router (ADR 0215): the generated contract validates requests and responses, the ctrl
//! manifest's capabilities gate the channels, runs act through the fixture helper, and the history survives a
//! restart.
use qrtz_app::ports::*;
use qrtz_app::*;
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::service::{CtrlService, SystemClock};
use qrtz_ipc::Router;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

/// The ctrl crate's fixture world: Safari, Notes, Finder, System Settings, Mail and Slack.
fn world() -> FakeWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../qrtz-ctrl/tests/fixtures");
    let mut w = FakeWorld {
        default_browser: Some("com.apple.Safari".into()),
        frontmost: Some("com.apple.finder".into()),
        ..Default::default()
    };
    for name in ["safari", "notes", "finder", "settings", "mail", "slack"] {
        let a: FakeApp =
            serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap())
                .unwrap();
        if a.running {
            w.state.insert(a.bundle_id.clone(), a.start.clone());
        }
        w.apps.push(a);
    }
    w
}

fn app(dir: &Path, ctrl: Option<Option<Arc<FakeHelper>>>) -> Router<Shared> {
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
    if let Some(helper) = ctrl {
        let mut c = CtrlService::open(dir.join("ctrl"), Box::new(SystemClock::default()));
        if let Some(fake) = helper {
            c = c.with_helper(Arc::new(HelperClient::new(Box::new(fake))));
        }
        svc.set_ctrl(Arc::new(c));
    }
    Router::new(Shared(svc))
}

async fn call(r: &Router<Shared>, sub: &str, ch: &str, payload: Value) -> Value {
    r.dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":sub,"payload":payload}))
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_do_runs_lists_scrubs_and_survives_a_restart() {
    let d = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeHelper::new(world()));
    let r = app(d.path(), Some(Some(fake.clone())));
    assert_eq!(
        call(&r, "ctrl", "ctrl:runs", Value::Null).await["data"],
        json!([])
    );
    let v = call(&r, "ctrl", "ctrl:do", json!({"text": "open youtube"})).await;
    let v = &v["data"];
    assert_eq!(v["status"], "done", "{v:#}");
    assert_eq!(v["app"], "safari");
    let run = v["runId"].as_str().unwrap().to_string();
    let early = call(&r, "ctrl", "ctrl:run", json!({"runId": run, "at": 1})).await;
    assert_eq!(early["data"]["status"], "running", "{early}");

    let again = app(d.path(), Some(None));
    let runs = call(&again, "ctrl", "ctrl:runs", Value::Null).await;
    assert_eq!(runs["data"][0]["runId"], run.as_str(), "{runs}");
    assert_eq!(runs["data"][0]["status"], "done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gated_press_waits_for_confirm_and_stop_answers_with_the_status() {
    let d = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeHelper::new(world()));
    let r = app(d.path(), Some(Some(fake.clone())));
    let v = call(&r, "ctrl", "ctrl:do", json!({"text": "press send in mail"})).await;
    let v = &v["data"];
    assert_eq!(v["status"], "needs-confirm", "{v:#}");
    let run = v["runId"].as_str().unwrap();
    let gate = v["confirm"]["gateId"].as_str().unwrap();
    let denied = call(
        &r,
        "ctrl",
        "ctrl:confirm",
        json!({"runId": run, "gateId": gate, "answer": "deny"}),
    )
    .await;
    assert_eq!(denied["data"]["status"], "cancelled", "{denied}");
    let v = call(&r, "ctrl", "ctrl:do", json!({"text": "press send in mail"})).await;
    let run = v["data"]["runId"].as_str().unwrap();
    let stop = call(&r, "ctrl", "ctrl:stop", json!({"runId": run})).await;
    assert_eq!(stop["data"], json!({"runId": run, "status": "stopped"}), "{stop}");
}

#[tokio::test]
async fn ctrl_channels_are_validated_gated_and_honest_when_missing() {
    let d = tempfile::tempdir().unwrap();
    let r = app(d.path(), Some(None));
    let empty = call(&r, "ctrl", "ctrl:do", json!({"text": 7})).await;
    assert_eq!(empty["error"]["code"], "E_VALIDATION", "{empty}");
    // Other sub apps don't hold ctrl's capabilities.
    assert_eq!(
        call(&r, "talk", "ctrl:runs", Value::Null).await["error"]["code"],
        "E_FORBIDDEN"
    );
    assert_eq!(
        call(&r, "ctrl", "ctrl:run", json!({"runId": "run-9"})).await["error"]["code"],
        "E_NOT_FOUND"
    );
    // No helper: a do is unavailable, not a silent no-op.
    let v = call(&r, "ctrl", "ctrl:do", json!({"text": "open youtube"})).await;
    assert_eq!(v["error"]["code"], "E_UNAVAILABLE", "{v}");
    let none = app(d.path(), None);
    let v = call(&none, "ctrl", "ctrl:runs", Value::Null).await;
    assert_eq!(v["error"]["code"], "E_UNAVAILABLE", "{v}");
}

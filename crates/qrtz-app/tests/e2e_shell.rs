//! Phase 5 port of e2e/smoke.spec.ts, e2e/shell.spec.ts (backend half), e2e/states.spec.ts "helper missing" and the
//! backend half of e2e/large-a11y.spec.ts "cold start" (docs/e2e-parity.md rows 1, 13, 30, 31).
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// smoke: boots, talks to the backend over typed IPC; a sub-app without the capability is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smoke_boots_and_serves_typed_ipc_with_capability_scoping() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("data"), Opts::default()).await;
    a.helper_ready().await;
    // Settings shows "database: ok · schema v5 · fts5 on" and "native helper: running".
    let h = a.settings("db:health", Value::Null).await;
    assert_eq!(h, json!({"ok": true, "schemaVersion": 5, "fts5": true}));
    let st = a.settings("helper:status", Value::Null).await;
    assert_eq!(st["state"], "running", "{st}");
    let info = a.call("shell", "app:get-info", Value::Null).await;
    assert_eq!(info["name"], "qrtz", "{info}");
    assert_eq!(a.note("note:list", json!({})).await, json!([]));
    // Capability scoping: "gallery" (no capabilities) may not read config.
    let denied = a.raw("gallery", "config:get", Value::Null).await;
    assert_eq!(denied["ok"], false, "{denied}");
    assert_eq!(denied["error"]["code"], "E_FORBIDDEN", "{denied}");
    // An unknown sub-app and a malformed envelope are refused too, never dispatched.
    let unknown = a.raw("evil", "config:get", Value::Null).await;
    assert_eq!(unknown["ok"], false, "{unknown}");
    let bad =
        a.r.dispatch(
            json!({"v":1,"id":"x","channel":"note:start","version":1,"subApp":"note","payload":{"title": 7}}),
        )
        .await;
    assert_eq!(bad["ok"], false, "{bad}");
    a.quit().await;
}

/// shell: the palette starts and stops a recording through shell-scoped IPC, and the shell scope cannot write notes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_palette_starts_and_stops_a_recording_through_shell_scoped_ipc() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("data"), Opts::default()).await;
    a.helper_ready().await;
    // First run: an empty meeting list, recording available.
    assert_eq!(a.note("note:list", json!({})).await, json!([]));
    assert_eq!(a.session().await["phase"], "idle");
    let s = a.call("shell", "note:start", json!({})).await;
    assert_eq!(s["phase"], "recording", "{s}");
    let stop = a.call("shell", "note:stop", Value::Null).await;
    assert!(
        matches!(stop["phase"].as_str(), Some("stopping" | "processing" | "done")),
        "{stop}"
    );
    a.until_phase("done").await;
    // The shell holds note.record/read, not note.write.
    let id = s["meetingId"].as_str().unwrap();
    let w = a.raw("shell", "note:delete", json!({"id": id})).await;
    assert_eq!(w["error"]["code"], "E_FORBIDDEN", "{w}");
    a.quit().await;
}

/// states: helper missing → health `missing`, recording refused with a clear error, the rest still works.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_missing_refuses_recording_and_the_app_stays_usable() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("data"), Opts::default().helper(Helper::Missing)).await;
    let s = a.session().await;
    assert_eq!(s["helper"], "missing", "{s}");
    let r = a.raw("note", "note:start", json!({})).await;
    let msg = if r["ok"] == true {
        r["data"]["error"].as_str().unwrap_or("").to_owned()
    } else {
        r["error"]["message"].as_str().unwrap_or("").to_owned()
    };
    assert!(msg.contains("helper is missing"), "{r}");
    assert_ne!(a.session().await["phase"], "recording");
    assert_eq!(a.note("note:list", json!({})).await, json!([]));
    // Settings has no error.
    assert_eq!(a.settings("db:health", Value::Null).await["ok"], true);
    a.settings("config:get", Value::Null).await;
    a.quit().await;
}

/// large-a11y "cold start" (backend half; first paint is the renderer's): boot to the first IPC answer within the
/// Electron budget (`QRTZ_COLD_START_BUDGET_MS`, 1500 ms).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_start_boot_to_first_ipc_answer_within_budget() {
    let budget: u64 = std::env::var("QRTZ_COLD_START_BUDGET_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    mock_helper_bin(); // build outside the measurement
    let tmp = tempfile::tempdir().unwrap();
    let t0 = Instant::now();
    let a = App::launch(&tmp.path().join("data"), Opts::default()).await;
    a.note("note:list", json!({})).await;
    a.session().await;
    let cold = t0.elapsed();
    eprintln!("{}", json!({"coldMs": cold.as_millis()}));
    assert!(cold < Duration::from_millis(budget), "cold start {cold:?}");
    a.quit().await;
}

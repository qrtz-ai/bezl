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

/// ADR 0146 M1.4: each module's settings page (inside its own view) reads and writes only its own settings; the
/// global settings page still sees everything through config:*.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_modules_settings_page_reaches_only_its_own_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let a = App::launch(&data, Opts::default()).await;
    let echo = a.call("talk", "module:get-settings", Value::Null).await;
    assert_eq!(echo["talk"]["showPill"], true, "{echo}");
    assert!(
        echo.get("note").is_none() && echo.get("theme").is_none(),
        "{echo}"
    );
    let after = a
        .call(
            "talk",
            "module:set-settings",
            json!({"patch": {"talk": {"showPill": false}}}),
        )
        .await;
    assert_eq!(
        after,
        json!({"talk": a.settings("config:get", Value::Null).await["talk"]})
    );
    assert_eq!(after["talk"]["showPill"], false);
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(data.join("modules/echo/settings.json")).unwrap())
            .unwrap();
    assert_eq!(file["talk"]["showPill"], false, "stored in echo's own file");
    let nano = a
        .call(
            "note",
            "module:set-settings",
            json!({"patch": {"privacy": {"excludedApps": ["zoom"]}}}),
        )
        .await;
    assert_eq!(
        nano["privacy"],
        json!({"excludedApps": ["zoom"], "pauseHotkey": "CommandOrControl+Shift+P"})
    );
    assert_eq!(
        a.call("ctrl", "module:get-settings", Value::Null).await,
        json!({})
    );
    for (sub, patch) in [
        ("talk", json!({"note": {"autoRecord": true}})),
        ("talk", json!({"modules": {"enabled": false}})),
        ("note", json!({"talk": {"showPill": true}})),
        ("note", json!({"speech": {"autoUpdate": false}})),
        ("ctrl", json!({"theme": "light"})),
    ] {
        let r = a.raw(sub, "module:set-settings", json!({"patch": patch})).await;
        assert_eq!(r["error"]["code"], "E_FORBIDDEN", "{sub} {r}");
    }
    // Shell views have no module settings, and a view without the capability can't ask.
    let r = a.raw("settings", "module:get-settings", Value::Null).await;
    assert_eq!(r["error"]["code"], "E_FORBIDDEN", "{r}");
    let r = a.raw("gallery", "module:get-settings", Value::Null).await;
    assert_eq!(r["error"]["code"], "E_FORBIDDEN", "{r}");
    let cfg = a.settings("config:get", Value::Null).await;
    assert_eq!(cfg["note"]["autoRecord"], false);
    assert_eq!(cfg["modules"]["enabled"], true);
    a.quit().await;
}

/// ADR 0161: the module manager switches one extension on or off through `packages:set-enabled` (the marketplace
/// has no config.write), and nothing else in config moves. Turning one on while "all modules" was off (the old
/// settings switch) leaves only that one on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_module_manager_switches_one_extension_and_nothing_else() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("data"), Opts::default()).await;
    let r = a
        .call(
            "marketplace",
            "packages:set-enabled",
            json!({"name": "ctrl", "enabled": false}),
        )
        .await;
    assert_eq!(r, json!({"name": "ctrl", "enabled": false}));
    let cfg = a.settings("config:get", Value::Null).await;
    assert_eq!(cfg["modules"]["ctrl"]["enabled"], false);
    assert_eq!(cfg["modules"]["echo"]["enabled"], true);
    assert_eq!(cfg["modules"]["enabled"], true);
    // A switched-off module's view is refused; the marketplace itself is not.
    let r = a.raw("ctrl", "ctrl:tasks", Value::Null).await;
    assert_eq!(r["error"]["code"], "E_UNAVAILABLE", "{r}");
    // The marketplace can't write config directly, and a module view can't switch modules.
    let r = a
        .raw("marketplace", "config:set", json!({"patch": {"theme": "light"}}))
        .await;
    assert_eq!(r["error"]["code"], "E_FORBIDDEN", "{r}");
    let r = a
        .raw(
            "talk",
            "packages:set-enabled",
            json!({"name": "ctrl", "enabled": true}),
        )
        .await;
    assert_eq!(r["error"]["code"], "E_FORBIDDEN", "{r}");
    let r = a
        .raw(
            "marketplace",
            "packages:set-enabled",
            json!({"name": "weather", "enabled": true}),
        )
        .await;
    assert_eq!(r["error"]["code"], "E_VALIDATION", "{r}");
    // Under the old "all modules: off", turning nano on leaves only nano on.
    a.settings("config:set", json!({"patch": {"modules": {"enabled": false}}}))
        .await;
    let r = a
        .call(
            "marketplace",
            "packages:set-enabled",
            json!({"name": "nano", "enabled": true}),
        )
        .await;
    assert_eq!(r, json!({"name": "nano", "enabled": true}));
    let cfg = a.settings("config:get", Value::Null).await;
    assert_eq!(cfg["modules"]["enabled"], true);
    assert_eq!(cfg["modules"]["echo"]["enabled"], false);
    assert_eq!(cfg["modules"]["ctrl"]["enabled"], false);
    a.quit().await;
}

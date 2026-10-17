//! Phase 5 port of e2e/talk.spec.ts (docs/e2e-parity.md rows 34, 36–38; row 35, the cleanup timeout, is retired:
//! talk has no LLM cleanup, `tests/talk.rs` `talk_never_calls_an_llm_even_with_an_openrouter_key`). The scripted helper plays the
//! TS mock's dictation ("um so I I think we should ship cue arts talk on friday", growing partials, insert.result).
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};
use std::sync::Arc;

fn stored(data: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(data.join("talk.json")).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn talk_live_streaming_text_cleanup_and_dictionary_insert_history_and_latency() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let storage = Arc::new(RecStorage::default());
    let a = App::launch_with(&data, Opts::default().helper(Helper::Scripted), storage.clone()).await;
    a.helper_ready().await;
    // Dictionary: qrtz = cue arts.
    let d = a
        .talk(
            "talk:set-dictionary",
            json!({"entries": [{"term": "qrtz", "aliases": ["cue arts"]}]}),
        )
        .await;
    assert_eq!(d[0]["term"], "qrtz", "{d}");
    assert_eq!(a.talk("talk:dictionary", Value::Null).await, d);

    // Text streams in a bit at a time: an early snapshot is a strict prefix of a later one.
    a.talk("talk:start", Value::Null).await;
    let early = talk_text(&a.until_talk("first words", |v| !talk_text(v).is_empty()).await);
    let v = a
        .until_talk("heard friday", |v| talk_text(v).contains("friday"))
        .await;
    let later = talk_text(&v);
    assert!(later.len() > early.len(), "{early:?} → {later:?}");
    assert!(
        later.starts_with(early.trim_end_matches('…')),
        "{early:?} → {later:?}"
    );
    assert_eq!(v["phase"], "listening", "{v}");
    // The level meter is live (what the pill and the talk view draw).
    a.until_talk("level", |v| v["level"].as_f64().unwrap_or(0.0) > 0.0)
        .await;
    a.talk("talk:stop", Value::Null).await;
    a.until_talk("idle", |v| v["phase"] == "idle").await;

    let h = a.talk_history().await;
    assert_eq!(h.len(), 1, "{h:?}");
    let e = &h[0];
    // No LLM cleanup (Jared): the on-device rules only (fillers, punctuation, the dictionary).
    assert_eq!(e["text"], "So I think we should ship qrtz talk on friday.", "{e}");
    assert_eq!(e["app"], "Mock Editor", "{e}");
    assert_eq!(
        (e["cleanup"].clone(), e["insert"].clone(), e["engine"].clone()),
        (json!("rules"), json!("ax-selected-text"), json!("on-device")),
        "{e}"
    );
    assert!(e["latency"]["firstPartialMs"].as_f64().unwrap() > 0.0, "{e}");
    assert!(e["latency"]["insertedMs"].as_f64().unwrap() >= 0.0, "{e}");
    // Persisted in talk.json with the same fields.
    assert_eq!(&stored(&data)["history"][0], e);

    // Copy puts the text on the clipboard; delete removes it.
    let id = e["id"].clone();
    a.talk("talk:copy", json!({"id": id})).await;
    assert_eq!(
        storage.clip().as_deref(),
        Some("So I think we should ship qrtz talk on friday.")
    );
    a.talk("talk:delete-entry", json!({"id": id})).await;
    assert!(a.talk_history().await.is_empty());
    assert_eq!(stored(&data)["history"], json!([]));
    a.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn talk_insertion_falls_back_to_paste_and_to_the_clipboard_when_nothing_can_insert() {
    let tmp = tempfile::tempdir().unwrap();
    let pdata = tmp.path().join("paste");
    let a = App::launch(
        &pdata,
        Opts::default()
            .helper(Helper::Scripted)
            .env("QRTZ_MOCK_INSERT", "paste"),
    )
    .await;
    a.helper_ready().await;
    let e = a.dictate().await;
    assert_eq!(e["insert"], "paste", "{e}");
    assert_eq!(e["inserted"], true, "{e}");
    assert_eq!(stored(&pdata)["history"][0]["insert"], "paste");
    a.quit().await;

    let storage = Arc::new(RecStorage::default());
    let b = App::launch_with(
        &tmp.path().join("fail"),
        Opts::default()
            .helper(Helper::Scripted)
            .env("QRTZ_MOCK_INSERT", "fail"),
        storage.clone(),
    )
    .await;
    b.helper_ready().await;
    let e = b.dictate().await;
    // "copied to clipboard": nothing could insert, so the text went to the clipboard.
    assert_eq!(e["inserted"], false, "{e}");
    assert_eq!(e["insert"], "none", "{e}");
    assert!(
        e["error"].as_str().unwrap_or("").ends_with("copied to clipboard"),
        "{e}"
    );
    let want = "So I think we should ship cue arts talk on friday.";
    assert_eq!(e["text"], want, "{e}");
    assert_eq!(storage.clip().as_deref(), Some(want));
    b.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn talk_cloud_engine_primary_with_automatic_failover_to_on_device_when_the_stream_dies() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(
        &tmp.path().join("ok"),
        Opts::default().helper(Helper::Scripted).talk_cloud("echo"),
    )
    .await;
    a.helper_ready().await;
    a.settings(
        "secrets:set",
        json!({"name": "asr.elevenlabs", "value": "el-test-not-real"}),
    )
    .await;
    a.settings("config:set", json!({"patch": {"talk": {"engine": "elevenlabs"}}}))
        .await;
    let e = a.dictate().await;
    assert_eq!(
        (e["engine"].clone(), e["failover"].clone()),
        (json!("elevenlabs"), json!(false)),
        "{e}"
    );
    a.quit().await;

    let b = App::launch(
        &tmp.path().join("bad"),
        Opts::default().helper(Helper::Scripted).talk_cloud("fail"),
    )
    .await;
    b.helper_ready().await;
    b.settings(
        "secrets:set",
        json!({"name": "asr.elevenlabs", "value": "el-test-not-real"}),
    )
    .await;
    b.settings("config:set", json!({"patch": {"talk": {"engine": "elevenlabs"}}}))
        .await;
    let e = b.dictate().await;
    // "on-device (fallback)".
    assert_eq!(
        (e["engine"].clone(), e["failover"].clone()),
        (json!("on-device"), json!(true)),
        "{e}"
    );
    b.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn talk_settings_has_a_key_for_every_automatic_behavior_and_show_pill_off_persists() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let a = App::launch(&data, Opts::default().helper(Helper::Scripted)).await;
    a.helper_ready().await;
    // Every toggle in settings is a config key with its default.
    let cfg = a.settings("config:get", Value::Null).await;
    for (k, on) in [
        ("enabled", true),
        ("autoFailover", true),
        ("warmMic", false),
        ("autoCleanup", true),
        ("autoInsert", true),
        ("restoreClipboard", true),
        ("showPill", true),
        ("saveHistory", true),
        ("blockGlobeKey", true),
    ] {
        assert_eq!(cfg["talk"][k], on, "talk.{k}: {cfg}");
    }
    assert_eq!(cfg["talk"]["engine"], "auto", "{cfg}");
    // Picking ElevenLabs (the key field shows) and turning the pill off persist.
    a.settings(
        "config:set",
        json!({"patch": {"talk": {"engine": "elevenlabs", "showPill": false}}}),
    )
    .await;
    a.quit().await;
    let b = App::launch(&data, Opts::default().helper(Helper::Scripted)).await;
    b.helper_ready().await;
    let cfg = b.call("shell", "config:get", Value::Null).await;
    assert_eq!(
        (cfg["talk"]["engine"].clone(), cfg["talk"]["showPill"].clone()),
        (json!("elevenlabs"), json!(false)),
        "{cfg}"
    );
    // With the pill off a dictation still streams in the talk view (the shell's `policy::pill_wanted` reads
    // talk.showPill and opens no #pill window; that half is apps/desktop/src-tauri/src/policy.rs + renderer).
    b.settings("config:set", json!({"patch": {"talk": {"engine": "on-device"}}}))
        .await;
    b.talk("talk:start", Value::Null).await;
    b.until_talk("streams with the pill off", |v| talk_text(v).contains("so"))
        .await;
    b.talk("talk:stop", Value::Null).await;
    b.until_talk("idle", |v| v["phase"] == "idle").await;
    b.quit().await;
}

/// Every `talk:state` the shell sees, with the time it arrived, and what the pill does for it
/// (`qrtz_app::pill::pill_action`, the rule `backend.rs::on_talk` applies).
/// Every `talk:state` view with the pill action it maps to, as a subscriber task sees them.
type PillTrace = Arc<std::sync::Mutex<Vec<(std::time::Instant, Value, qrtz_app::pill::PillAction)>>>;

fn pill_trace(a: &App, config: Value) -> PillTrace {
    let out = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut rx = a.app.service.events.subscribe();
    let o = out.clone();
    tokio::spawn(async move {
        while let Ok((ev, v)) = rx.recv().await {
            if ev == "talk:state" {
                let act = qrtz_app::pill::pill_action(&config, &v);
                o.lock().unwrap().push((std::time::Instant::now(), v, act));
            }
        }
    });
    out
}

/// Jared: the pill goes the instant the key is released; the final text finishes in the background and is
/// inserted when ready (no LLM cleanup any more). The very first view after the release hides the pill, before
/// the text is in, and nothing shows it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pill_hides_on_release_and_the_text_is_inserted_later_in_the_background() {
    use qrtz_app::pill::PillAction;
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("d"), Opts::default().helper(Helper::Scripted)).await;
    a.helper_ready().await;
    let trace = pill_trace(&a, json!({"talk": {"showPill": true}}));
    a.talk("talk:start", Value::Null).await;
    a.until_talk("heard friday", |v| talk_text(v).contains("friday"))
        .await;
    assert!(
        trace
            .lock()
            .unwrap()
            .iter()
            .any(|(_, _, p)| *p == PillAction::Show),
        "shown while held"
    );
    let released = std::time::Instant::now();
    let n = trace.lock().unwrap().len();
    a.talk("talk:stop", Value::Null).await;
    let e = wait("history entry", || async {
        a.talk_history().await.first().cloned()
    })
    .await;
    assert_eq!(e["inserted"], true, "{e}");
    let inserted = std::time::Instant::now();
    let after: Vec<_> = trace.lock().unwrap()[n..].to_vec();
    let (first_at, first, act) = after.first().cloned().expect("a view after the release");
    assert_ne!(first["phase"], "listening", "{first}");
    assert_eq!(act, PillAction::Hide, "{first}");
    assert!(
        first_at.duration_since(released) < std::time::Duration::from_millis(250),
        "hidden {:?} after the release",
        first_at.duration_since(released)
    );
    assert!(first_at < inserted, "hidden before the text was in");
    assert!(
        after
            .iter()
            .any(|(_, v, _)| v["phase"] == "inserting" || v["phase"] == "finalizing"),
        "the final text and insert ran after the release: {after:?}"
    );
    assert!(
        after.iter().all(|(_, _, p)| *p == PillAction::Hide),
        "never shown again: {after:?}"
    );
    a.quit().await;
}

/// A dictation that fails after the release (nothing could insert; the text went to the clipboard) never held
/// the pill open: it hid on release and flashes the error once afterwards, or notifies with the pill off.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failure_after_release_flashes_the_error_once_afterwards() {
    use qrtz_app::pill::PillAction;
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(
        &tmp.path().join("d"),
        Opts::default()
            .helper(Helper::Scripted)
            .env("QRTZ_MOCK_INSERT", "fail"),
    )
    .await;
    a.helper_ready().await;
    let on = pill_trace(&a, json!({"talk": {"showPill": true}}));
    let off = pill_trace(&a, json!({"talk": {"showPill": false}}));
    a.talk("talk:start", Value::Null).await;
    a.until_talk("heard friday", |v| talk_text(v).contains("friday"))
        .await;
    a.talk("talk:stop", Value::Null).await;
    let e = a.until_talk("talk idle", |v| v["phase"] == "idle").await;
    assert_eq!(e["last"]["inserted"], false, "{e}");
    // The traces are fed by their own subscriber tasks, so wait on what they hold, not on a count taken earlier:
    // both have seen the failure's flash.
    let flashed = |t: &PillTrace| {
        t.lock()
            .unwrap()
            .iter()
            .any(|(_, _, p)| matches!(p, PillAction::Flash { .. }))
    };
    wait("the failure flashed (pill on and off)", || async {
        (flashed(&on) && flashed(&off)).then_some(())
    })
    .await;
    // Everything after the last listening view (the release onwards).
    let trace = on.lock().unwrap().clone();
    let release = trace
        .iter()
        .rposition(|(_, v, _)| v["phase"] == "listening")
        .expect("listening views were traced");
    let after: Vec<_> = trace[release + 1..].iter().map(|(_, _, p)| p.clone()).collect();
    assert!(!after.is_empty(), "{trace:?}");
    assert_eq!(after[0], PillAction::Hide, "hidden on release first: {after:?}");
    let flashes: std::collections::BTreeSet<String> = after
        .iter()
        .filter_map(|p| match p {
            PillAction::Flash { key, error, notify } => {
                assert!(error.ends_with("copied to clipboard"), "{error}");
                assert!(!notify);
                Some(key.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(flashes.len(), 1, "one failure, one flash key: {after:?}");
    assert!(!after.contains(&PillAction::Show), "{after:?}");
    assert!(
        off.lock()
            .unwrap()
            .iter()
            .any(|(_, _, p)| matches!(p, PillAction::Flash { notify: true, .. })),
        "with the pill off the failure is a notification"
    );
    a.quit().await;
}

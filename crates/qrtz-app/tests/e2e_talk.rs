//! Phase 5 port of e2e/talk.spec.ts (docs/e2e-parity.md rows 34, 36–38; row 35, the cleanup timeout, is Bobs's
//! `tests/talk.rs` `ai_cleanup_uses_openrouter_and_local_only_keeps_it_on_device`). The scripted helper plays the
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
    let a = App::launch_with(
        &data,
        Opts::default().helper(Helper::Scripted).talk_llm("fake"),
        storage.clone(),
    )
    .await;
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
    assert_eq!(e["text"], "I think we should ship qrtz talk on friday.", "{e}");
    assert_eq!(e["app"], "Mock Editor", "{e}");
    assert_eq!(
        (e["cleanup"].clone(), e["insert"].clone(), e["engine"].clone()),
        (json!("llm"), json!("ax-selected-text"), json!("on-device")),
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
        Some("I think we should ship qrtz talk on friday.")
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
        json!({"name": "asr.deepgram", "value": "dg-test-not-real"}),
    )
    .await;
    a.settings("config:set", json!({"patch": {"talk": {"engine": "deepgram"}}}))
        .await;
    let e = a.dictate().await;
    assert_eq!(
        (e["engine"].clone(), e["failover"].clone()),
        (json!("deepgram"), json!(false)),
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
        json!({"name": "asr.deepgram", "value": "dg-test-not-real"}),
    )
    .await;
    b.settings("config:set", json!({"patch": {"talk": {"engine": "deepgram"}}}))
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
        ("aiCleanup", true),
        ("autoInsert", true),
        ("restoreClipboard", true),
        ("showPill", true),
        ("saveHistory", true),
    ] {
        assert_eq!(cfg["talk"][k], on, "talk.{k}: {cfg}");
    }
    assert_eq!(cfg["talk"]["engine"], "auto", "{cfg}");
    // Picking Deepgram (the key field shows) and turning the pill off persist.
    a.settings(
        "config:set",
        json!({"patch": {"talk": {"engine": "deepgram", "showPill": false}}}),
    )
    .await;
    a.quit().await;
    let b = App::launch(&data, Opts::default().helper(Helper::Scripted)).await;
    b.helper_ready().await;
    let cfg = b.call("shell", "config:get", Value::Null).await;
    assert_eq!(
        (cfg["talk"]["engine"].clone(), cfg["talk"]["showPill"].clone()),
        (json!("deepgram"), json!(false)),
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

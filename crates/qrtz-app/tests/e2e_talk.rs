//! Phase 5 port of e2e/talk.spec.ts (docs/e2e-parity.md rows 34, 36, 38; row 35, the cleanup timeout, is retired:
//! talk has no LLM cleanup; row 37, the cloud engine with failover, is retired with the cloud, ADR 0076: an old
//! config that picked ElevenLabs dictates on-device). The scripted helper plays the
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
async fn an_old_config_that_picked_elevenlabs_dictates_on_device() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("config.json"),
        json!({"talk": {"engine": "elevenlabs", "autoFailover": false, "showPill": false}}).to_string(),
    )
    .unwrap();
    std::fs::write(data.join("secrets.json"), br#"{"asr.elevenlabs":"el-old"}"#).unwrap();
    let a = App::launch(&data, Opts::default().helper(Helper::Scripted)).await;
    a.helper_ready().await;
    let cfg = a.settings("config:get", Value::Null).await;
    assert!(cfg["talk"].get("engine").is_none(), "{cfg}");
    assert_eq!(cfg["talk"]["showPill"], false, "the rest still loads: {cfg}");
    assert!(
        !data.join("secrets.json").exists(),
        "the old key is deleted at launch"
    );
    let e = a.dictate().await;
    assert_eq!(
        (e["engine"].clone(), e["failover"].clone()),
        (json!("on-device"), json!(false)),
        "{e}"
    );
    a.quit().await;
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
    // Turning the pill off persists.
    a.settings("config:set", json!({"patch": {"talk": {"showPill": false}}}))
        .await;
    a.quit().await;
    let b = App::launch(&data, Opts::default().helper(Helper::Scripted)).await;
    b.helper_ready().await;
    let cfg = b.call("shell", "config:get", Value::Null).await;
    assert_eq!(cfg["talk"]["showPill"], false, "{cfg}");
    // With the pill off a dictation still streams in the talk view (the shell's `policy::pill_wanted` reads
    // talk.showPill and opens no #pill window; that half is apps/desktop/src-tauri/src/policy.rs + renderer).
    b.talk("talk:start", Value::Null).await;
    b.until_talk("streams with the pill off", |v| talk_text(v).contains("so"))
        .await;
    b.talk("talk:stop", Value::Null).await;
    b.until_talk("idle", |v| v["phase"] == "idle").await;
    b.quit().await;
}

/// Every `talk:state` the shell sees, with the time it arrived, and what the pill does for it
/// (`qrtz_app::pill::PillGate`, the rule `backend.rs::on_talk` applies: each failure flashes once).
type PillTrace = Arc<std::sync::Mutex<Vec<(std::time::Instant, Value, qrtz_app::pill::PillAction)>>>;

fn pill_trace(a: &App, config: Value) -> PillTrace {
    let out = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut rx = a.app.service.events.subscribe();
    let o = out.clone();
    tokio::spawn(async move {
        let mut gate = qrtz_app::pill::PillGate::default();
        while let Ok((ev, v)) = rx.recv().await {
            if ev == "talk:state" {
                let act = gate.next(&config, &v);
                o.lock().unwrap().push((std::time::Instant::now(), v, act));
            }
        }
    });
    out
}

/// Jared (Oct 17): the pill stays up after the key is released, through the final text and the insert, and
/// hides once the text is in. Nothing hides it early, and nothing shows it again afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pill_stays_up_until_the_text_is_inserted_then_hides() {
    use qrtz_app::pill::PillAction;
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("d"), Opts::default().helper(Helper::Scripted)).await;
    a.helper_ready().await;
    let trace = pill_trace(&a, json!({"talk": {"showPill": true}}));
    a.talk("talk:start", Value::Null).await;
    a.until_talk("heard friday", |v| talk_text(v).contains("friday"))
        .await;
    let n = trace.lock().unwrap().len();
    a.talk("talk:stop", Value::Null).await;
    let e = wait("history entry", || async {
        a.talk_history().await.first().cloned()
    })
    .await;
    assert_eq!(e["inserted"], true, "{e}");
    let after = wait("the pill hid", || async {
        let t = trace.lock().unwrap()[n..].to_vec();
        t.iter().any(|(_, v, _)| v["phase"] == "idle").then_some(t)
    })
    .await;
    let done = after.iter().position(|(_, v, _)| v["phase"] == "idle").unwrap();
    assert!(done > 0, "finalizing / inserting views came first: {after:?}");
    for (_, v, p) in &after[..done] {
        assert_eq!(*p, PillAction::Show, "shown while busy: {v}");
    }
    assert!(
        after[done..].iter().all(|(_, _, p)| *p == PillAction::Hide),
        "hidden once the text is in, and stays hidden: {after:?}"
    );
    a.quit().await;
}

/// A dictation that fails after the release (nothing could insert; the text went to the clipboard): the pill stays
/// up while it finishes, then flashes the error once, or notifies with the pill off.
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
    let first_flash = after
        .iter()
        .position(|p| matches!(p, PillAction::Flash { .. }))
        .expect("flashed");
    assert!(
        after[..first_flash].iter().all(|p| *p == PillAction::Show),
        "shown until the failure: {after:?}"
    );
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
    assert!(
        !after[first_flash..].contains(&PillAction::Show),
        "not shown again after the flash: {after:?}"
    );
    assert!(
        off.lock()
            .unwrap()
            .iter()
            .any(|(_, _, p)| matches!(p, PillAction::Flash { notify: true, .. })),
        "with the pill off the failure is a notification"
    );
    a.quit().await;
}

/// The push key as the helper reports it (`hotkey` down / up), into the app like a helper line.
fn key(a: &App, action: &str) {
    a.app
        .service
        .on_helper_line(&json!({"type": "hotkey", "action": action}).to_string());
}

/// `<type> <id>` lines the scripted helper received (`QRTZ_MOCK_CMD_LOG`).
fn received(log: &std::path::Path, ty: &str) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix(&format!("{ty} ")).map(str::to_owned))
        .collect()
}

/// Review §12.22 (b), N110 (ADR 0074): a second press while the first dictation's paste is still out (a slow
/// `insert.result`) starts a new dictation at once: the pill shows again, and both texts go in, in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_press_during_a_slow_insert_gives_two_ordered_inserts_and_shows_the_pill_again() {
    use qrtz_app::pill::PillAction;
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("cmds.log");
    let a = App::launch(
        &tmp.path().join("d"),
        Opts::default()
            .helper(Helper::Scripted)
            .env("QRTZ_MOCK_INSERT_DELAY_MS", "1500")
            .env("QRTZ_MOCK_CMD_LOG", log.to_str().unwrap()),
    )
    .await;
    a.helper_ready().await;
    let trace = pill_trace(&a, json!({"talk": {"showPill": true}}));

    key(&a, "down");
    let first = a
        .until_talk("first heard friday", |v| {
            v["phase"] == "listening" && talk_text(v).contains("friday")
        })
        .await;
    let first_id = first["id"].as_str().unwrap().to_owned();
    key(&a, "up");
    a.until_talk("first inserting", |v| v["phase"] == "inserting")
        .await;
    assert_eq!(
        received(&log, "insert.text"),
        std::slice::from_ref(&first_id),
        "the first paste is out"
    );
    let shown_before = trace.lock().unwrap().len();

    key(&a, "down");
    let second = a
        .until_talk("second listening", |v| {
            v["phase"] == "listening" && v["id"] != first_id.as_str()
        })
        .await;
    let second_id = second["id"].as_str().unwrap().to_owned();
    assert!(
        a.talk_history()
            .await
            .iter()
            .all(|e| e["id"] != first_id.as_str()),
        "the second dictation started before the first insert finished"
    );
    a.until_talk("second heard friday", |v| {
        v["id"] == second_id.as_str() && talk_text(v).contains("friday")
    })
    .await;
    key(&a, "up");

    let h = wait("both entries", || async {
        let h = a.talk_history().await;
        (h.len() == 2).then_some(h)
    })
    .await;
    for e in &h {
        assert_eq!(e["inserted"], true, "{e}");
    }
    assert_eq!(received(&log, "insert.text"), [first_id, second_id], "in order");
    assert!(
        trace.lock().unwrap()[shown_before..]
            .iter()
            .any(|(_, _, p)| *p == PillAction::Show),
        "the pill came back for the second dictation"
    );
    let last = trace.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.2, PillAction::Hide, "{:?}", last.1);
    a.quit().await;
}

/// Review §12.22 (b), N111 (ADR 0074): a 20 ms tap while the engine is still starting (a slow
/// `dictation.started`) leaves nothing running: the helper is told to stop the dictation (the mic closes), the
/// view ends idle, and the pill is hidden from the release on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_20ms_tap_during_a_slow_engine_start_leaves_the_mic_closed_and_the_pill_hidden_when_done() {
    use qrtz_app::pill::PillAction;
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("cmds.log");
    let a = App::launch(
        &tmp.path().join("d"),
        Opts::default()
            .helper(Helper::Scripted)
            .env("QRTZ_MOCK_DICTATION_START_MS", "400")
            .env("QRTZ_MOCK_CMD_LOG", log.to_str().unwrap()),
    )
    .await;
    a.helper_ready().await;
    let trace = pill_trace(&a, json!({"talk": {"showPill": true}}));
    key(&a, "down");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    key(&a, "up");

    let started = wait("dictation.start sent", || async {
        received(&log, "dictation.start").first().cloned()
    })
    .await;
    wait("the helper was told to stop it", || async {
        (received(&log, "dictation.stop").contains(&started)
            || received(&log, "dictation.cancel").contains(&started))
        .then_some(())
    })
    .await;
    // Past the slow start and well past any late `started`: still nothing listening.
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let v = a.talk_state().await;
    assert_ne!(v["phase"], "listening", "{v}");
    a.until_talk("idle", |v| v["phase"] == "idle").await;
    // The pill shows on the press itself (it doesn't wait for the engine), so it may blink for the tap; once the
    // tap's dictation is done it is hidden and nothing (not the late start, not the empty result) brings it back.
    let t = trace.lock().unwrap().clone();
    let done = t.iter().position(|(_, v, _)| v["phase"] == "idle").expect("done");
    assert!(
        t[done..].iter().all(|(_, _, p)| *p == PillAction::Hide),
        "the pill stayed hidden after the tap: {t:?}"
    );
    // The next press still works (nothing is stuck).
    key(&a, "down");
    a.until_talk("listening again", |v| v["phase"] == "listening")
        .await;
    key(&a, "up");
    a.until_talk("idle again", |v| v["phase"] == "idle").await;
    a.quit().await;
}

/// Regression (Jared, Oct 17: "pressing fn does nothing" after the offline change): his real config.json from
/// before ADR 0076 (cloud keys, `engine: auto`, `privacy.localOnly`, `speech.cloud`, the removed chat/cleanup
/// keys, no `blockGlobeKey`) and no speech module (the Mac build: `SpeechSetup` is `None`, the helper's own
/// recogniser is the on-device engine). Push key down → listening, the pill shown, the helper told to consume Fn;
/// key up → the text is inserted on device and a history entry is written; the pill hides when done.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_push_key_dictates_on_device_with_a_pre_offline_mac_config_and_no_speech_module() {
    use qrtz_app::pill::PillAction;
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let log = tmp.path().join("cmds.log");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("config.json"),
        json!({
            "app": {"autoUpdateCheck": true, "launchAtLogin": false, "menuBarIcon": true},
            "logLevel": "info",
            "note": {"autoDetectMeetings": true, "autoRecord": false, "autoSummarize": true,
                     "backgroundIndexing": true, "defaultTemplate": "general", "detectPrompt": true,
                     "floatingHud": true, "llmModel": "", "llmProvider": "openrouter", "notifyNoAudio": true,
                     "retainAudioDays": 0, "searchModelDownload": true},
            "onboarding": {"done": true},
            "privacy": {"excludedApps": [], "localOnly": false, "pauseHotkey": "CommandOrControl+Shift+P"},
            "schemaVersion": 1,
            "speech": {"autoUpdate": true, "cloud": "off", "speakerMode": "fast"},
            "talk": {"aiCleanup": true, "autoCleanup": true, "autoFailover": true, "autoInsert": true,
                     "cleanupModel": "", "cleanupTimeoutMs": 900, "enabled": true, "engine": "auto",
                     "preRollMs": 300, "pushKey": "fn", "restoreClipboard": true, "saveHistory": true,
                     "showPill": true, "toggleHotkey": "CommandOrControl+Shift+Space", "warmMic": false},
            "telemetry": {"crashReports": false},
            "theme": "dark"
        })
        .to_string(),
    )
    .unwrap();
    let a = App::launch(
        &data,
        Opts::default()
            .helper(Helper::Scripted)
            .env("QRTZ_MOCK_CMD_LOG", log.to_str().unwrap()),
    )
    .await;
    a.helper_ready().await;
    let cfg = a.settings("config:get", Value::Null).await;
    assert_eq!(cfg["talk"]["enabled"], true, "{cfg}");
    assert_eq!(cfg["talk"]["pushKey"], "fn", "{cfg}");
    let trace = pill_trace(&a, cfg.clone());
    key(&a, "down");
    a.until_talk("listening on device", |v| {
        v["phase"] == "listening" && v["engine"] == "on-device" && talk_text(v).contains("friday")
    })
    .await;
    key(&a, "up");
    let e = wait("history entry", || async {
        a.talk_history().await.first().cloned()
    })
    .await;
    assert_eq!(e["inserted"], true, "{e}");
    assert_eq!(e["engine"], "on-device", "{e}");
    assert!(
        !received(&log, "insert.text").is_empty(),
        "the text went to the focused app"
    );
    wait("the pill hid when done", || async {
        let t = trace.lock().unwrap().clone();
        (t.iter().any(|(_, _, p)| *p == PillAction::Show)
            && t.last()
                .is_some_and(|(_, v, p)| v["phase"] == "idle" && *p == PillAction::Hide))
        .then_some(())
    })
    .await;
    a.quit().await;
}

/// The helper couldn't watch the push key (`error{dictation.hotkey}`, no Input Monitoring / Accessibility):
/// pressing fn would do nothing, so the user is told (talk error view → the pill flashes it), never silence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unavailable_push_key_is_shown_not_silent() {
    use qrtz_app::pill::PillAction;
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("d"), Opts::default().helper(Helper::Scripted)).await;
    a.helper_ready().await;
    let trace = pill_trace(&a, json!({"talk": {"showPill": true}}));
    a.app.service.on_helper_line(
        &json!({"type": "error", "code": "dictation.hotkey",
                "message": "push-to-talk key fn unavailable (Input Monitoring / Accessibility?)", "fatal": false})
        .to_string(),
    );
    let v = a.until_talk("error shown", |v| v["phase"] == "error").await;
    assert!(
        v["error"].as_str().unwrap_or("").contains("push-to-talk key"),
        "{v}"
    );
    wait("flashed", || async {
        trace
            .lock()
            .unwrap()
            .iter()
            .any(|(_, _, p)| matches!(p, PillAction::Flash { .. }))
            .then_some(())
    })
    .await;
    a.quit().await;
}

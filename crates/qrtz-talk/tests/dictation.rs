//! Dictation scenarios against the scripted helper and the speech module seam.
mod common;
use common::*;
use qrtz_talk::mock::MOCK_DICTATION;
use qrtz_talk::*;
use serde_json::json;

#[tokio::test(start_paused = true)]
async fn on_device_is_the_only_engine() {
    let r = rig(RigOpts::default());
    let v = r.dictate(400).await;
    let e = v.last.expect("an entry");
    assert_eq!(e.engine, Engine::OnDevice);
    assert!(!e.failover);
    assert!(e.inserted);
    assert!(
        !e.raw.is_empty() && MOCK_DICTATION.starts_with(&e.raw),
        "raw = {}",
        e.raw
    );
    assert_eq!(r.phases(), ["listening", "finalizing", "inserting", "idle"]);
    assert_eq!(r.svc.history().len(), 1);
    // The helper never streams the mic to the host: there is no host-side (cloud) engine (ADR 0076).
    assert!(r
        .helper
        .sent_of("dictation.configure")
        .iter()
        .all(|m| m["streamAudio"] == false));
}

#[tokio::test(start_paused = true)]
async fn an_old_cloud_config_still_dictates_on_device() {
    // A config from before ADR 0076: ElevenLabs picked, failover off, local-only on. All of it is ignored.
    let r = rig(RigOpts {
        config: json!({"privacy": {"localOnly": true},
                       "talk": {"engine": "elevenlabs", "autoFailover": false}}),
        ..Default::default()
    });
    assert_eq!(r.svc.resolve_engine(), Engine::OnDevice);
    let v = r.dictate(300).await;
    let e = v.last.unwrap();
    assert_eq!(e.engine, Engine::OnDevice);
    assert!(!e.failover);
    assert!(v.error.is_none());
}

#[tokio::test(start_paused = true)]
async fn helper_dies_mid_dictation_never_sticks_in_listening() {
    let r = rig(RigOpts::default());
    let me = r.svc.clone();
    tokio::spawn(async move { me.start().await });
    until(
        || r.view().phase == TalkPhase::Listening && !r.view().partial.is_empty(),
        1000,
    )
    .await;
    r.helper.die();
    until(|| !r.view().active(), 2000).await;
    let v = r.view();
    assert_eq!(v.phase, TalkPhase::Error);
    assert_eq!(v.error_kind, Some(TalkErrorKind::HelperLost));
    // A new helper comes back and dictation works again.
    r.helper.restart();
    sleep(20).await;
    let v = r.dictate(300).await;
    assert!(v.last.is_some());
}

#[tokio::test(start_paused = true)]
async fn busy_cancels_only_the_orphan_never_the_view() {
    let r = rig(RigOpts::default());
    r.helper.preload_dictation("orphan-1");
    let me = r.svc.clone();
    tokio::spawn(async move { me.start().await });
    until(|| r.view().phase == TalkPhase::Error, 1000).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::Busy));
    let cancels = r.helper.sent_of("dictation.cancel");
    assert_eq!(cancels.len(), 1);
    assert_eq!(
        cancels[0]["id"], "orphan-1",
        "cancels the orphan activeId, not our id"
    );
    // The helper is free again: the next dictation works.
    let v = r.dictate(300).await;
    assert!(v.last.is_some());
}

#[tokio::test(start_paused = true)]
async fn busy_for_someone_elses_start_is_ignored() {
    let r = rig(RigOpts::default());
    let me = r.svc.clone();
    tokio::spawn(async move { me.start().await });
    until(|| r.view().phase == TalkPhase::Listening, 1000).await;
    r.helper.module_line(json!({"type": "error", "code": "dictation.busy", "message": "busy", "id": "not-ours", "activeId": "t1"}));
    sleep(100).await;
    assert_eq!(r.view().phase, TalkPhase::Listening);
    assert!(
        r.helper.sent_of("dictation.cancel").is_empty(),
        "never cancels by the view's id"
    );
    r.svc.stop().await;
    until(|| !r.view().active(), 2000).await;
    assert!(r.view().last.is_some());
}

#[tokio::test(start_paused = true)]
async fn mic_revoked_mid_dictation_stops_and_keeps_what_was_heard() {
    let r = rig(RigOpts::default());
    let me = r.svc.clone();
    tokio::spawn(async move { me.start().await });
    until(|| r.view().partial.split(' ').count() >= 3, 1000).await;
    r.helper.revoke_mic();
    until(|| r.view().phase == TalkPhase::Error, 2000).await;
    let v = r.view();
    assert_eq!(v.error_kind, Some(TalkErrorKind::PermissionRevoked));
    let e = r.last_entry().expect("heard words are inserted and saved");
    assert!(!e.raw.is_empty());
}

#[tokio::test(start_paused = true)]
async fn mic_denied_is_typed() {
    let r = rig(RigOpts {
        helper: HelperOpts {
            mic_denied: true,
            audio_every_ms: 50,
            ..Default::default()
        },
        ..Default::default()
    });
    let me = r.svc.clone();
    tokio::spawn(async move { me.start().await });
    until(|| r.view().phase == TalkPhase::Error, 1000).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::MicDenied));
}

/// N134: a start the helper refuses for want of a recogniser fails at once, typed, without a cancel (it never ran).
#[tokio::test(start_paused = true)]
async fn no_recogniser_is_typed_and_immediate() {
    let r = rig(RigOpts {
        helper: HelperOpts {
            no_recogniser: true,
            audio_every_ms: 50,
            ..Default::default()
        },
        ..Default::default()
    });
    let me = r.svc.clone();
    tokio::spawn(async move { me.start().await });
    until(|| r.view().phase == TalkPhase::Error, 500).await;
    let v = r.view();
    assert_eq!(v.error_kind, Some(TalkErrorKind::SpeechUnavailable));
    assert_eq!(
        v.error.as_deref(),
        Some("speech recognition isn't ready yet; try again in a moment")
    );
    assert!(!r.helper.sent().iter().any(|m| m["type"] == "dictation.cancel"));
}

#[tokio::test(start_paused = true)]
async fn recording_and_dictation_share_the_mic() {
    let r = rig(RigOpts::default());
    let rec = r.helper.recording_start("/tmp/x.caf");
    assert!(matches!(rec.decision, qrtz_lifecycle::Decision::Accept));
    let v = r.dictate(300).await;
    assert!(v.last.is_some(), "dictation runs while a recording holds the mic");
    assert!(
        r.helper.arbiter().recording,
        "the recording is untouched by dictation ending"
    );
    let rec = r.helper.recording_stop();
    assert!(matches!(rec.decision, qrtz_lifecycle::Decision::Accept));
}

#[tokio::test(start_paused = true)]
async fn warm_mic_is_configured_on_hello() {
    let r = rig(RigOpts {
        config: json!({"talk": {"warmMic": true, "preRollMs": 500}}),
        ..Default::default()
    });
    r.helper.restart();
    sleep(20).await;
    let c = r.helper.sent_of("dictation.configure");
    let last = c.last().expect("configure sent");
    assert_eq!(last["warmMic"], true);
    assert_eq!(last["preRollMs"], 500);
    assert_eq!(last["consumeFn"], true, "talk.blockGlobeKey is on by default");
    assert_eq!(
        last["streamAudio"], false,
        "no audio streaming: there is no host-side engine"
    );
    assert!(r.helper.arbiter().warm);
}

#[tokio::test(start_paused = true)]
async fn insert_failure_lands_on_the_clipboard() {
    let r = rig(RigOpts {
        helper: HelperOpts {
            insert_fails: true,
            audio_every_ms: 50,
            ..Default::default()
        },
        ..Default::default()
    });
    let v = r.dictate(300).await;
    let e = v.last.unwrap();
    assert!(!e.inserted);
    assert!(e.error.unwrap().contains("copied to clipboard"));
    assert_eq!(r.clip.0.lock().unwrap().last().unwrap(), &format!("{} ", e.text));
    assert!(!e.text.ends_with(' '), "the history entry has no trailing space");
}

#[tokio::test(start_paused = true)]
async fn the_pasted_text_ends_in_a_space_and_the_history_text_does_not() {
    let r = rig(RigOpts::default());
    let v = r.dictate(400).await;
    let e = v.last.expect("an entry");
    assert!(e.inserted);
    assert!(
        !e.text.is_empty() && !e.text.ends_with(char::is_whitespace),
        "history: {:?}",
        e.text
    );
    let ins = r.helper.sent_of("insert.text");
    assert_eq!(ins.len(), 1);
    assert_eq!(ins[0]["text"].as_str().unwrap(), format!("{} ", e.text));
    let saved = r.svc.history();
    assert_eq!(
        saved.last().unwrap().text,
        e.text,
        "the saved transcript is unchanged"
    );
}

#[test]
fn the_trailing_space_is_added_once_and_never_to_nothing() {
    assert_eq!(service::with_trailing_space("hi there."), "hi there. ");
    assert_eq!(service::with_trailing_space(""), "");
    assert_eq!(service::with_trailing_space("hi "), "hi ");
    assert_eq!(service::with_trailing_space("hi\n"), "hi\n");
}

#[tokio::test(start_paused = true)]
async fn ipc_channels_round_trip() {
    let r = rig(RigOpts::default());
    let st = r.svc.call("talk:state", json!({})).await.unwrap();
    assert_eq!(st["phase"], "idle");
    let d = r
        .svc
        .call(
            "talk:set-dictionary",
            json!({"entries": [{"term": " qrtz ", "aliases": ["cue arts"]}, {"term": "qrtz"}]}),
        )
        .await
        .unwrap();
    assert_eq!(
        d,
        json!([{"term": "qrtz", "aliases": ["cue arts"]}]),
        "trimmed and de-duplicated like TS"
    );
    let v = r.dictate(900).await;
    let e = v.last.unwrap();
    assert!(e.text.contains("qrtz"), "dictionary applied: {}", e.text);
    let h = r.svc.call("talk:history", json!({})).await.unwrap();
    assert_eq!(h[0]["id"], e.id.as_str());
    assert_eq!(
        r.svc.call("talk:copy", json!({"id": e.id})).await.unwrap()["ok"],
        true
    );
    assert_eq!(
        r.svc
            .call("talk:delete-entry", json!({"id": e.id}))
            .await
            .unwrap()["ok"],
        true
    );
    assert!(r.svc.history().is_empty());
    assert!(r.svc.call("talk:nope", json!({})).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn the_view_carries_a_typed_error_kind_on_the_wire() {
    let r = rig(RigOpts {
        helper: HelperOpts {
            mic_denied: true,
            audio_every_ms: 50,
            ..Default::default()
        },
        ..Default::default()
    });
    let v = r.svc.call("talk:start", json!({})).await.unwrap();
    assert_eq!(v["errorKind"], "mic-denied");
}

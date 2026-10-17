//! Dictation scenarios against the scripted helper, the speech module seam and the mock cloud.
mod common;
use common::*;
use qrtz_talk::mock::{MockCloud, MOCK_DICTATION};
use qrtz_talk::*;
use serde_json::json;

fn el() -> serde_json::Value {
    json!({"talk": {"engine": "elevenlabs"}})
}

#[tokio::test(start_paused = true)]
async fn on_device_is_the_default_and_never_touches_the_network() {
    let r = rig(RigOpts::default()); // an ElevenLabs key exists, but the cloud is opt-in
    let v = r.dictate(400).await;
    assert!(
        r.cloud.requests().is_empty(),
        "no cloud connection without opting in"
    );
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
}

#[tokio::test(start_paused = true)]
async fn on_device_works_with_local_only_and_no_keys() {
    let r = rig(RigOpts {
        elevenlabs: false,
        config: json!({"privacy": {"localOnly": true}, "talk": {"engine": "elevenlabs"}}),
        ..Default::default()
    });
    let v = r.dictate(300).await;
    assert!(r.cloud.requests().is_empty());
    assert_eq!(v.last.unwrap().engine, Engine::OnDevice);
}

#[tokio::test(start_paused = true)]
async fn elevenlabs_when_selected_streams_in_host_and_wins() {
    let r = rig(RigOpts {
        config: el(),
        ..Default::default()
    });
    let v = r.dictate(400).await;
    assert_eq!(r.cloud.engines(), [Engine::Elevenlabs]);
    assert!(
        r.cloud.audio_frames(0) > 0,
        "helper audio is forwarded to the cloud"
    );
    let e = v.last.unwrap();
    assert_eq!(e.engine, Engine::Elevenlabs);
    assert!(!e.failover);
    assert_eq!(e.raw, MOCK_DICTATION);
    // The key is a header on the request only.
    let req = &r.cloud.requests()[0];
    assert!(req.headers.iter().any(|(k, v)| k == "xi-api-key" && v == KEY_EL));
    assert!(!req.url.contains(KEY_EL));
    assert!(!r.everything_said().contains(KEY_EL));
    let real = qrtz_talk::ws::WsRequest {
        url: req.url.clone(),
        headers: req.headers.clone(),
        protocols: vec![KEY_EL.into()],
    };
    assert!(
        !format!("{real:?}").contains(KEY_EL),
        "Debug of a request redacts headers and subprotocols"
    );
}

#[tokio::test(start_paused = true)]
async fn cloud_drop_mid_stream_fails_over_to_on_device() {
    let r = rig(RigOpts {
        config: el(),
        cloud: MockCloud::new().with(Engine::Elevenlabs, MockMode::DropAfter(3)),
        ..Default::default()
    });
    let v = r.dictate(600).await;
    let e = v.last.expect("dictation still lands");
    assert_eq!(e.engine, Engine::OnDevice);
    assert!(e.failover);
    assert!(e.inserted);
    assert!(!e.raw.is_empty());
    assert!(v.error.is_none());
}

#[tokio::test(start_paused = true)]
async fn cloud_down_at_start_fails_over_with_auto_failover() {
    let r = rig(RigOpts {
        config: el(),
        cloud: MockCloud::new().with(Engine::Elevenlabs, MockMode::Refuse),
        ..Default::default()
    });
    let v = r.dictate(400).await;
    let e = v.last.unwrap();
    assert_eq!(e.engine, Engine::OnDevice);
    assert!(e.failover);
}

#[tokio::test(start_paused = true)]
async fn cloud_down_without_failover_is_a_fast_typed_offline_error() {
    let r = rig(RigOpts {
        config: json!({"talk": {"engine": "elevenlabs", "autoFailover": false}}),
        cloud: MockCloud::new().with(Engine::Elevenlabs, MockMode::Refuse),
        ..Default::default()
    });
    let me = r.svc.clone();
    tokio::spawn(async move { me.start().await });
    until(|| r.view().phase == TalkPhase::Error, 200).await;
    let v = r.view();
    assert_eq!(v.error_kind, Some(TalkErrorKind::Offline));
    assert!(!v.error.unwrap_or_default().contains(KEY_EL));
    assert!(
        r.helper.sent_of("dictation.cancel").len() == 1,
        "the mic is released"
    );
    assert!(
        r.svc.history().is_empty(),
        "never substitutes on-device text for the cloud the user picked"
    );
    assert!(!r.everything_said().contains(KEY_EL));
}

#[tokio::test(start_paused = true)]
async fn cloud_lost_mid_dictation_without_failover_keeps_heard_words_on_clipboard() {
    let cloud = MockCloud::new();
    let r = rig(RigOpts {
        config: json!({"talk": {"engine": "elevenlabs", "autoFailover": false}}),
        cloud: cloud.clone(),
        ..Default::default()
    });
    let me = r.svc.clone();
    tokio::spawn(async move { me.start().await });
    until(|| !r.view().text.is_empty() || !r.view().partial.is_empty(), 1000).await;
    sleep(200).await;
    cloud.set_offline(true);
    until(|| r.view().phase == TalkPhase::Error, 200).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::Offline));
    let clip = r.clip.0.lock().unwrap().clone();
    assert_eq!(clip.len(), 1);
    assert!(MOCK_DICTATION.starts_with(&clip[0]) && !clip[0].is_empty());
    assert!(r.svc.history().is_empty());
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
        "no audio streaming without an opted-in cloud"
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
    assert_eq!(r.clip.0.lock().unwrap().last().unwrap(), &e.text);
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

#[test]
fn provider_errors_are_redacted() {
    let m = qrtz_talk::session::redact(
        &format!("401 for key {KEY_EL} at wss://x?token={KEY_EL}"),
        Some(KEY_EL),
    );
    assert!(!m.contains(KEY_EL));
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

//! Phase 5 port of e2e/permissions.spec.ts (docs/e2e-parity.md rows 25–27; row 28, N29/N32, is in e2e_capture.rs):
//! H5 TCC revoked mid-capture degrades the recording live and stops a dictation gracefully; H6 a dictation runs
//! alongside a meeting recording (mux, shared mic tap) and leaves the recording untouched.
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};

fn degraded(s: &Value, what: &str) -> bool {
    s["degraded"]
        .as_array()
        .is_some_and(|d| d.iter().any(|x| x == what))
}

async fn stop_to_done(a: &App, id: &str) -> Value {
    a.note("note:stop", Value::Null).await;
    a.until_phase("done").await;
    a.until_settled(id).await;
    a.note("note:get", json!({"id": id})).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h5_screen_then_mic_revoked_mid_recording_goes_mic_only_then_no_audio_and_keeps_recording() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(
        &tmp.path().join("data"),
        Opts::default()
            .standup("0.2")
            .env("QRTZ_MOCK_REVOKE", "screen-recording@800,microphone@2000"),
    )
    .await;
    a.helper_ready().await;
    a.until_phase("detected").await;
    let id = a.note("note:start", json!({})).await["meetingId"]
        .as_str()
        .unwrap()
        .to_owned();
    a.until_phase("recording").await;
    let s = a.until_session("mic-only", |s| degraded(s, "mic-only")).await;
    assert_eq!(s["phase"], "recording", "{s}");
    let s = a
        .until_session("audio-unavailable", |s| degraded(s, "audio-unavailable"))
        .await;
    assert_eq!(s["phase"], "recording", "still recording: {s}");
    assert_eq!(s["meetingId"], json!(id));
    let d = stop_to_done(&a, &id).await;
    assert_eq!(d["meeting"]["status"], "done", "{d}");
    a.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h6_dictation_runs_during_a_meeting_recording_and_the_recording_is_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("data"), Opts::default().standup("0.2")).await;
    a.helper_ready().await;
    a.until_phase("detected").await;
    let id = a.note("note:start", json!({})).await["meetingId"]
        .as_str()
        .unwrap()
        .to_owned();
    a.until_phase("recording").await;
    // Dictate while recording (the arbiter muxes the mic tap).
    let e = a.dictate().await;
    assert!(e["text"].as_str().unwrap().contains("friday"), "{e}");
    // The recording carried on: same meeting, still recording, live transcript flowing.
    let s = a.session().await;
    assert_eq!(
        (s["phase"].clone(), s["meetingId"].clone()),
        (json!("recording"), json!(id)),
        "{s}"
    );
    wait("live transcript after the dictation", || async {
        let t = a.note("note:live-transcript", json!({})).await;
        (segments(&t) > 0).then_some(())
    })
    .await;
    let d = stop_to_done(&a, &id).await;
    assert!(!d["segments"].as_array().unwrap().is_empty(), "{d}");
    assert!(d["meeting"]["audioPath"].is_string(), "{d}");
    // The dictation is not part of the meeting transcript.
    assert!(!d["segments"].to_string().contains("cue arts talk"), "{d}");
    a.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h5_microphone_revoked_mid_dictation_stops_gracefully_keeping_the_text_and_the_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(
        &tmp.path().join("data"),
        Opts::default()
            .helper(Helper::Scripted)
            .env("QRTZ_MOCK_REVOKE", "microphone@400"),
    )
    .await;
    a.helper_ready().await;
    a.talk("talk:start", Value::Null).await;
    let v = a
        .until_talk("dictation stopped by the revocation", |v| {
            v["errorKind"] == "permission-revoked"
        })
        .await;
    assert!(
        v["error"]
            .as_str()
            .unwrap_or("")
            .contains("microphone access was turned off"),
        "{v}"
    );
    // The words heard so far are kept as a history entry.
    let h = wait("entry kept", || async {
        let h = a.talk_history().await;
        (!h.is_empty()).then_some(h)
    })
    .await;
    assert!(!h[0]["raw"].as_str().unwrap_or("").is_empty(), "{h:?}");
    a.quit().await;
}

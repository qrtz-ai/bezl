#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Phase 5 port of e2e/capture.spec.ts and the N29/N32 case of e2e/permissions.spec.ts (docs/e2e-parity.md rows
//! 2, 3, 4, 28) against the Rust `qrtz-mock-helper` (QRTZ_MOCK_SILENT / QRTZ_MOCK_MIC_FAILS).
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};

fn silent(s: &Value, src: &str) -> Option<Value> {
    s["silent"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["source"] == src).cloned())
}

fn no_audio(s: &Value) -> bool {
    s["noAudio"] == true
}

async fn recording(env: &[(&str, &str)]) -> (tempfile::TempDir, App, String) {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = Opts::default();
    for (k, v) in env {
        o = o.env(k, v);
    }
    let a = App::launch(&tmp.path().join("data"), o).await;
    a.helper_ready().await;
    let s = a.note("note:start", json!({})).await;
    assert_eq!(s["phase"], "recording", "{s}");
    let id = s["meetingId"].as_str().unwrap().to_owned();
    (tmp, a, id)
}

async fn stop_done(a: &App, id: &str) -> Value {
    a.note("note:stop", Value::Null).await;
    a.until_phase("done").await;
    a.until_settled(id).await
}

/// N29: system audio goes silent → a live per-source flag with its start (the badge's time of day), cleared when
/// audio returns; never the stronger no-audio state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n29_system_silent_shows_live_then_clears_when_audio_returns() {
    let (_t, a, id) = recording(&[("QRTZ_MOCK_SILENT", "system@1000-3500")]).await;
    let s = a
        .until_session("system silent", |s| silent(s, "system").is_some())
        .await;
    assert!(silent(&s, "system").unwrap()["sinceMs"].is_number(), "{s}");
    assert!(!no_audio(&s), "{s}");
    assert!(silent(&s, "mic").is_none(), "{s}");
    let s = a
        .until_session("system audible", |s| silent(s, "system").is_none())
        .await;
    assert_eq!(s["phase"], "recording", "{s}");
    let p = stop_done(&a, &id).await;
    // The span stays on the meeting's capture record, closed.
    let spans = p["capture"]["silentSpans"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        spans
            .iter()
            .any(|x| x["source"] == "system" && x["endMs"].is_number()),
        "{p}"
    );
    a.quit().await;
}

/// N29: every source silent → noAudio ("no audio is being recorded"); the finished meeting keeps both silences.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n29_every_source_silent_is_no_audio_and_the_meeting_keeps_the_notice() {
    let (_t, a, id) = recording(&[("QRTZ_MOCK_SILENT", "system@800,mic@800")]).await;
    a.until_session("noAudio", no_audio).await;
    let p = stop_done(&a, &id).await;
    let spans = p["capture"]["silentSpans"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for src in ["system", "mic"] {
        assert!(spans.iter().any(|x| x["source"] == src), "{src}: {p}");
    }
    // The capture record survives in the meeting (the "capture notice" after opening it).
    let again = a.note("note:pipeline", json!({"id": id})).await;
    assert_eq!(again["capture"], p["capture"]);
    a.quit().await;
}

/// N38: the mic fails at start, in the same chunk as recording.started → recording continues with "no mic".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n38_mic_fails_at_start_and_recording_continues_with_no_mic() {
    let (_t, a, id) = recording(&[("QRTZ_MOCK_MIC_FAILS", "start")]).await;
    let s = a
        .until_session("no-mic", |s| {
            s["degraded"]
                .as_array()
                .is_some_and(|d| d.contains(&json!("no-mic")))
        })
        .await;
    assert_eq!(s["phase"], "recording", "{s}");
    let p = stop_done(&a, &id).await;
    assert_eq!(p["steps"]["finalize"]["status"], "done", "{p}");
    a.quit().await;
}

/// N29/N32: a silent mic shows live and clears when audible again; with every source silent the state is noAudio
/// (the per-source mic badge gives way to it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n29_n32_silent_mic_clears_when_audible_and_all_silent_is_no_audio() {
    let (_t, a, id) = recording(&[("QRTZ_MOCK_SILENT", "mic@600-2500")]).await;
    a.until_session("mic silent", |s| silent(s, "mic").is_some())
        .await;
    a.until_session("mic audible", |s| silent(s, "mic").is_none())
        .await;
    stop_done(&a, &id).await;
    a.quit().await;

    let (_t2, b, id) = recording(&[("QRTZ_MOCK_SILENT", "system@600,mic@600")]).await;
    let s = b.until_session("noAudio", no_audio).await;
    assert_eq!(s["phase"], "recording", "{s}");
    stop_done(&b, &id).await;
    b.quit().await;
}

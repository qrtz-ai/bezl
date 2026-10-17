//! Phase 5 port of e2e/crash.spec.ts (docs/e2e-parity.md row 5): the helper dies mid-meeting; the supervisor
//! restarts it, the recording resumes into a second WAV part, and at stop the parts are stitched (header repaired)
//! and the meeting is summarized. Nothing from before the crash is lost and the session never sticks.
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_crash_mid_recording_reconnects_resumes_stitches_audio_and_keeps_the_transcript() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let mark = tmp.path().join("crashed");
    let a = App::launch(
        &data,
        Opts::default()
            .standup("0.04")
            .env("QRTZ_MOCK_CRASH_AFTER_MS", "900")
            .env("QRTZ_MOCK_CRASH_MARK", mark.to_str().unwrap())
            // The Electron suite's QRTZ_RETRY_SCALE (0.05): the reconnect deadline scales with it, and the
            // supervisor's restart has to fit inside it.
            .retry_scale(0.05),
    )
    .await;
    let mut bus = a.app.service.events.subscribe();
    a.helper_ready().await;
    a.until_phase("detected").await;
    let s = a.note("note:start", json!({})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();

    // The helper dies ~0.9 s in, comes back, and capture resumes.
    let mut phases: Vec<String> = vec![];
    wait("reconnecting then recording again", || {
        while let Ok((name, p)) = bus.try_recv() {
            if name == "note:session" {
                phases.push(p["phase"].as_str().unwrap_or("").to_owned());
            }
        }
        assert!(
            !phases.iter().any(|p| p == "done"),
            "stopped instead of resuming: {phases:?}"
        );
        let r = phases.iter().position(|p| p == "reconnecting");
        let ok = r.is_some_and(|i| phases[i..].iter().any(|p| p == "recording"));
        async move { ok.then_some(()) }
    })
    .await;
    assert!(mark.exists());
    a.note("note:stop", Value::Null).await;
    a.until_phase("done").await;
    let p = a.until_settled(&id).await;
    assert_eq!(p["steps"]["finalize"]["status"], "done", "{p}");
    let list = a.note("note:list", json!({})).await;
    assert_ne!(list[0]["meeting"]["status"], "recording", "{list}");
    let d = a.note("note:get", json!({"id": id})).await;
    assert!(d["summary"]["summary"].is_string(), "{d}");
    assert!(!d["segments"].as_array().unwrap().is_empty(), "{d}");

    // One recording: the resumed part was stitched into the first and removed; the header is repaired.
    let recs: Vec<_> = std::fs::read_dir(data.join("recordings"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(recs.len(), 1, "{recs:?}");
    let wav = std::fs::read(&recs[0]).unwrap();
    let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
    assert_eq!(data_len, wav.len() - 44);
    assert!(wav.len() > 100_000, "{}", wav.len());
    a.quit().await;
}

#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Phase 5 port of e2e/note.spec.ts (docs/e2e-parity.md row 18): detect → record (fixture audio + ASR stream with AX
//! speaker names) → live transcript → HUD notes → stop → extractive summary → list/search → export → restart →
//! palette search. Key-free (no provider configured).
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn note_end_to_end_detect_record_live_transcript_summarize_search_export() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let export_dir = tmp.path().join("export");
    std::fs::create_dir_all(&export_dir).unwrap();
    let storage = Arc::new(RecStorage::default());
    *storage.export_dir.lock().unwrap() = Some(export_dir.clone());
    let a = App::launch_with(&data, Opts::default().standup("0.04"), storage.clone()).await;
    a.helper_ready().await;

    // 1. The helper detects a Zoom meeting.
    let s = a.until_phase("detected").await;
    assert_eq!(s["detected"]["title"], "Weekly Product Sync", "{s}");
    assert_eq!(s["detected"]["platform"], "zoom", "{s}");
    assert_eq!(s["detected"]["app"], "zoom.us", "{s}");

    // 2. Record → live transcript with AX speaker names.
    let s = a.note("note:start", json!({})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();
    a.until_session("active speaker", |s| s["activeSpeaker"].is_string())
        .await;
    wait("Grace Hopper's line and the last line live", || async {
        let t = a.note("note:live-transcript", json!({})).await;
        let grace = t["segments"]
            .as_array()
            .is_some_and(|s| s.iter().any(|x| x["speaker"] == "Grace Hopper"));
        (grace && has_text(&t, "let's ship the desktop launch next week")).then_some(())
    })
    .await;
    // The mini HUD (a shell window) is asked to expand through the backend.
    assert_eq!(
        a.call("shell", "hud:set-expanded", json!({"expanded": true}))
            .await,
        json!({"expanded": true})
    );
    // HUD notes: stamped with the recording time.
    let n = a
        .note("note:add-note", json!({"text": "From the mini HUD"}))
        .await;
    assert_eq!(n["meetingId"], json!(id), "{n}");
    // Fixture audio was written for the recording.
    let recs: Vec<_> = std::fs::read_dir(data.join("recordings"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(recs.len(), 1);
    assert!(std::fs::metadata(&recs[0]).unwrap().len() > 1_000_000);

    // 3. Stop → the on-device summary (ADR 0075: the helper's Apple Intelligence model; here the mock's) with the
    //    action item it found.
    a.note("note:stop", Value::Null).await;
    a.until_phase("done").await;
    a.until_settled(&id).await;
    let d = a.note("note:get", json!({"id": id})).await;
    let sum = &d["summary"];
    assert_eq!(sum["provider"], "apple-intelligence", "{sum}");
    assert_eq!(
        sum["summary"], "mock on-device single summary of 8 fenced lines.",
        "{sum}"
    );
    assert_eq!(sum["actionItems"][0]["owner"], "Alan Turing", "{sum}");
    assert!(sum.get("llmError").is_none_or(Value::is_null), "{sum}");
    assert_eq!(d["notes"][0]["text"], "From the mini HUD", "{d}");
    assert!(d["notes"][0]["atMs"].is_number());
    // Transcript: 8 speaker-labelled lines.
    let segs = d["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 8, "{d}");
    assert!(
        segs.iter()
            .any(|s| s["speakerName"] == "Alan Turing" || s["speaker"] == "Alan Turing"),
        "{d}"
    );

    // 4. Meeting list + full-text search.
    assert_eq!(a.note("note:list", json!({})).await.as_array().unwrap().len(), 1);
    let hits = a.note("note:list", json!({"query": "parakeet"})).await;
    assert!(
        hits[0]["hit"]["snippet"]
            .as_str()
            .unwrap_or("")
            .contains("[parakeet]"),
        "{hits}"
    );
    assert_eq!(a.note("note:list", json!({"query": "zebra"})).await, json!([]));
    assert_eq!(
        a.note("note:list", json!({"query": ""}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // 5. Markdown export to a file.
    let ex = a.note("note:export", json!({"id": id, "save": true})).await;
    let files: Vec<_> = std::fs::read_dir(&export_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{ex}");
    let name = files[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(name.ends_with("weekly-product-sync.md"), "{name}");
    let md = std::fs::read_to_string(&files[0]).unwrap();
    for want in [
        "# Weekly Product Sync",
        "## Action items",
        "- [ ] ",
        "**[0:04] Grace Hopper:**",
    ] {
        assert!(md.contains(want), "{want} in {md}");
    }
    a.quit().await;

    // Persistence: the meeting survives a restart; ⌘K meeting search (FTS) finds it.
    let b = App::launch(&data, Opts::default()).await;
    let list = b.note("note:list", json!({})).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["meeting"]["title"], "Weekly Product Sync");
    let hit = b.call("shell", "note:list", json!({"query": "onboarding"})).await;
    assert_eq!(hit.as_array().unwrap().len(), 1, "{hit}");
    assert_eq!(hit[0]["meeting"]["title"], "Weekly Product Sync", "{hit}");
    b.quit().await;
}

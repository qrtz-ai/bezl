//! Phase 5 port of e2e/chat.spec.ts (docs/e2e-parity.md rows 19, 20; row 21 is in e2e_scale.rs): speaker rename and
//! merge, per-meeting and cross-meeting chat (key-free, extractive with sources), N90 (a meeting pending delete is
//! invisible to chat on every retrieval path), and the "Always record" setting.
mod e2e_support;
use e2e_support::*;
use qrtz_search::LazyEmbedder;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

fn speakers(d: &Value) -> BTreeSet<String> {
    d["segments"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["speakerName"].as_str().map(str::to_owned))
        .collect()
}

fn cites(answer: &Value, id: &str) -> bool {
    answer["sources"]
        .as_array()
        .is_some_and(|s| s.iter().any(|x| x["meetingId"] == id))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_per_meeting_and_across_meetings_speaker_rename_merge_and_pending_delete_is_invisible() {
    let tmp = tempfile::tempdir().unwrap();
    // A ready (fake) embedder: the index step writes vectors, so chat runs the hybrid path (FTS + closest lines).
    let a = App::launch(
        &tmp.path().join("data"),
        Opts::default()
            .standup("0.02")
            .embedder(LazyEmbedder::ready_with(Arc::new(FakeEmbedder))),
    )
    .await;
    a.helper_ready().await;
    a.until_phase("detected").await;
    // Stop only after the "parakeet latency" line the cross-meeting question needs.
    let (id, p) = record_until(&a, None, |t| segments(t) > 5 && has_text(t, "parakeet")).await;
    assert_eq!(p["steps"]["index"]["status"], "done", "{p}");

    // Speaker rename, then merge (a rename onto an existing name).
    let d = a.note("note:get", json!({"id": id})).await;
    let before = speakers(&d);
    assert!(before.len() > 1, "{before:?}");
    let first = before.iter().next().unwrap().clone();
    let r = a
        .note(
            "note:rename-speaker",
            json!({"meetingId": id, "from": first, "to": "Ada (PM)"}),
        )
        .await;
    assert!(r["changed"].as_u64().unwrap() > 0, "{r}");
    let after = speakers(&a.note("note:get", json!({"id": id})).await);
    assert!(after.contains("Ada (PM)") && !after.contains(&first), "{after:?}");
    assert_eq!(after.len(), before.len());
    let other = after.iter().find(|s| *s != "Ada (PM)").unwrap().clone();
    a.note(
        "note:rename-speaker",
        json!({"meetingId": id, "from": other, "to": "Ada (PM)"}),
    )
    .await;
    let d = a.note("note:get", json!({"id": id})).await;
    let merged = speakers(&d);
    assert_eq!(merged.len(), before.len() - 1, "{merged:?}");
    assert!(!merged.contains(&other));

    // Per-meeting Q&A: no key, extractive answer with sources from this meeting.
    let ans = a
        .note(
            "note:chat",
            json!({"question": "Who will write the onboarding copy?", "meetingId": id}),
        )
        .await;
    assert!(
        ans["answer"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("onboarding"),
        "{ans}"
    );
    let srcs = ans["sources"].as_array().unwrap();
    assert!(!srcs.is_empty(), "{ans}");
    assert!(srcs.iter().all(|s| s["meetingId"] == json!(id)), "{ans}");
    assert!(srcs[0]["startMs"].is_number(), "{ans}");
    // A source opens the transcript at a segment of this meeting.
    let at = srcs[0]["startMs"].as_u64().unwrap();
    assert!(
        d["segments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["startMs"].as_u64() == Some(at)),
        "source {at} is a segment start: {d}"
    );

    // Across all meetings: the citation is this meeting.
    let all = a.note("note:chat", json!({"question": "parakeet latency"})).await;
    assert!(
        all["answer"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("parakeet"),
        "{all}"
    );
    assert!(cites(&all, &id), "{all}");
    assert_eq!(all["sources"][0]["title"], "Weekly Product Sync", "{all}");

    // N90: inside the undo window the meeting is invisible to chat (full text, closest lines, and a question scoped to it).
    a.note("note:delete", json!({"id": id, "deferMs": 60_000})).await;
    let hid = a.note("note:chat", json!({"question": "parakeet latency"})).await;
    assert!(
        !cites(&hid, &id),
        "pending delete leaked into cross-meeting chat: {hid}"
    );
    assert!(!hid.to_string().to_lowercase().contains("parakeet"), "{hid}");
    let scoped = a
        .raw(
            "note",
            "note:chat",
            json!({"question": "Who will write the onboarding copy?", "meetingId": id}),
        )
        .await;
    assert!(
        scoped["ok"] == false || !cites(&scoped["data"], &id),
        "pending delete leaked into scoped chat: {scoped}"
    );
    assert!(!scoped.to_string().contains("onboarding copy"), "{scoped}");
    // Undo: visible again.
    a.note("note:undo-delete", json!({"id": id})).await;
    let back = a.note("note:chat", json!({"question": "parakeet latency"})).await;
    assert!(cites(&back, &id), "{back}");
    a.quit().await;
}

fn auto_record(data: &std::path::Path) -> Value {
    let cfg: Value = serde_json::from_slice(&std::fs::read(data.join("config.json")).unwrap()).unwrap();
    cfg["note"]["autoRecord"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detected_meeting_prompts_and_always_record_persists_and_auto_starts_next_time() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let a = App::launch(&data, Opts::default().standup("0.05")).await;
    a.helper_ready().await;
    // The detection prompts (no auto-record yet): phase detected, nothing recording.
    let s = a.until_phase("detected").await;
    assert!(s["meetingId"].is_null(), "{s}");
    // "Always record": persist the setting, then record this one.
    assert_eq!(
        a.note("note:set-auto-record", json!({"enabled": true})).await,
        json!({"autoRecord": true})
    );
    a.note("note:start", json!({})).await;
    a.until_phase("recording").await;
    a.note("note:stop", Value::Null).await;
    a.until_phase("done").await;
    a.quit().await;
    assert_eq!(auto_record(&data), true);

    // Next launch: the same detection starts recording on its own (no prompt, no note:start).
    let b = App::launch(&data, Opts::default().standup("0.05")).await;
    b.helper_ready().await;
    let s = b.until_phase("recording").await;
    assert!(s["meetingId"].is_string(), "{s}");
    assert_eq!(
        b.call("shell", "config:get", Value::Null).await["note"]["autoRecord"],
        true
    );
    // Turning it off persists.
    assert_eq!(
        b.note("note:set-auto-record", json!({"enabled": false})).await,
        json!({"autoRecord": false})
    );
    b.note("note:stop", Value::Null).await;
    b.until_phase("done").await;
    b.quit().await;
    assert_eq!(auto_record(&data), false);
    let list_dir = data.clone();
    let c = App::launch(&list_dir, Opts::default()).await;
    assert_eq!(c.note("note:list", json!({})).await.as_array().unwrap().len(), 2);
    c.quit().await;
}

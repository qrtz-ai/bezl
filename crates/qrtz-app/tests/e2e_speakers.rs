//! Phase 5 port of e2e/chat.spec.ts (docs/e2e-parity.md rows 19, 20; row 21 is in e2e_scale.rs): speaker rename and
//! merge, full-text search hiding a meeting pending delete (N90), and the "Always record" setting. The spec's chat
//! part went with the "ask" chat (ADR 0072).
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn speakers(d: &Value) -> BTreeSet<String> {
    d["segments"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["speakerName"].as_str().map(str::to_owned))
        .collect()
}

fn finds(list: &Value, id: &str) -> bool {
    list.as_array()
        .is_some_and(|l| l.iter().any(|m| m["meeting"]["id"] == id))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn speaker_rename_merge_and_search_hides_a_meeting_pending_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("data"), Opts::default().standup("0.02")).await;
    a.helper_ready().await;
    a.until_phase("detected").await;
    // Stop only after the "parakeet latency" line the search needs.
    let (id, p) = record_until(&a, None, |t| segments(t) > 5 && has_text(t, "parakeet")).await;
    assert!(
        matches!(p["steps"]["index"]["status"].as_str(), Some("done" | "skipped")),
        "{p}"
    );

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

    // Full-text search finds the meeting by its transcript; inside the undo window it is invisible (N90).
    let q = json!({"query": "parakeet latency"});
    assert!(finds(&a.note("note:list", q.clone()).await, &id));
    a.note("note:delete", json!({"id": id, "deferMs": 60_000})).await;
    let hid = a.note("note:list", q.clone()).await;
    assert!(!finds(&hid, &id), "pending delete leaked into search: {hid}");
    // Undo: visible again.
    a.note("note:undo-delete", json!({"id": id})).await;
    assert!(finds(&a.note("note:list", q).await, &id));
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

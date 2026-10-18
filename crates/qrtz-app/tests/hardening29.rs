//! Hardening cycle 29 (N148, ADR 0094): a meeting whose delete is pending is not readable by id. Since ADR 0091 a
//! meeting is an address (`#/note/meetings/:id`) that a notification, the palette or the tab bar's remembered path
//! can open, so `note:get`, `note:export` and `note:list-notes` must agree with `note:list` and playback (N133):
//! hidden while the tombstone stands, readable again after Undo, gone after the window.
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};
use std::sync::Arc;

const READERS: [&str; 3] = ["note:get", "note:export", "note:list-notes"];

fn payload(ch: &str, id: &str) -> Value {
    match ch {
        "note:list-notes" => json!({"meetingId": id}),
        "note:export" => json!({"id": id, "save": false}),
        _ => json!({"id": id}),
    }
}

async fn listed(a: &App, id: &str) -> bool {
    a.note("note:list", json!({"limit": 1000}))
        .await
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["meeting"]["id"] == id)
}

/// A deep link opened inside the undo window (a "summary ready" notification for the meeting just deleted, ADR
/// 0091) finds nothing, with the code the renderer shows as "not found"; nothing reaches the clipboard. Undo makes
/// every reader work again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_meeting_pending_delete_reads_as_not_found_until_undo() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    seed_many(&data, 3);
    let storage = Arc::new(RecStorage::default());
    let a = App::launch_with(&data, Opts::default(), storage.clone()).await;
    let id = "00000000-0000-4000-9000-000000000001";
    let other = "00000000-0000-4000-9000-000000000002";
    a.note(
        "note:add-note",
        json!({"meetingId": id, "text": "private follow-up"}),
    )
    .await;
    for ch in READERS {
        assert_eq!(
            a.raw("note", ch, payload(ch, id)).await["ok"],
            true,
            "{ch} before the delete"
        );
    }
    let clip_before = storage.clip();

    a.note("note:delete", json!({"id": id, "deferMs": 60_000})).await;
    assert!(!listed(&a, id).await);
    for ch in READERS {
        let r = a.raw("note", ch, payload(ch, id)).await;
        assert_eq!(r["ok"], false, "{ch} served a meeting pending delete: {r}");
        assert_eq!(r["error"]["code"], "E_NOT_FOUND", "{ch}: {r}");
        assert!(!r.to_string().contains("private follow-up"), "{ch}: {r}");
    }
    assert_eq!(
        storage.clip(),
        clip_before,
        "export of a pending delete reached the clipboard"
    );
    // Another meeting is untouched.
    for ch in READERS {
        assert_eq!(
            a.raw("note", ch, payload(ch, other)).await["ok"],
            true,
            "{ch} other"
        );
    }

    assert_eq!(
        a.note("note:undo-delete", json!({"id": id})).await,
        json!({"restored": true})
    );
    assert!(listed(&a, id).await);
    for ch in READERS {
        assert_eq!(
            a.raw("note", ch, payload(ch, id)).await["ok"],
            true,
            "{ch} after undo"
        );
    }
    let notes = a.note("note:list-notes", json!({"meetingId": id})).await;
    assert!(notes.to_string().contains("private follow-up"), "{notes}");
    a.quit().await;
}

/// A quit inside the undo window keeps the tombstone; until the next launch completes it, the meeting stays
/// unreadable by id there too (the readers consult the same tombstones the list does), and afterwards it is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tombstone_from_the_last_launch_hides_the_meeting_then_it_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    seed_many(&data, 2);
    let a = App::launch(&data, Opts::default()).await;
    let id = "00000000-0000-4000-9000-000000000000";
    a.note("note:delete", json!({"id": id, "deferMs": 6_000})).await;
    a.quit().await;

    let b = App::launch(&data, Opts::default()).await;
    // Whether the launch has completed the tombstone yet or not, no reader serves the meeting: not found, or (the
    // notes of a meeting that no longer exists) an empty list.
    for ch in READERS {
        let r = b.raw("note", ch, payload(ch, id)).await;
        let empty = r["ok"] == true && r["data"] == json!([]);
        assert!(empty || r["error"]["code"] == "E_NOT_FOUND", "{ch}: {r}");
    }
    assert!(!listed(&b, id).await);
    b.quit().await;
}

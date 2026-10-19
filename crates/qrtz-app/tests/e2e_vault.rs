//! ADR 0220 phase 1 through the app router: the notes vault (`vault:*`, `doc:*`), standalone notes, meeting links,
//! the migration of old meeting notes (with its backup, once), the mini HUD's quick add, no silent overwrite, and
//! confinement (only nano's view reaches the vault).
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};

const M0: &str = "00000000-0000-4000-9000-000000000000";
const M1: &str = "00000000-0000-4000-9000-000000000001";

fn seed_notes(data: &std::path::Path) {
    seed_many(data, 2);
    seed_db(data, |repo| {
        for (i, t) in ["ask about pricing", "send the deck"].iter().enumerate() {
            let n: qrtz_model::MeetingNote = serde_json::from_value(
                json!({"id": format!("n{i}"), "atMs": 61_000 * (i as u64 + 1), "text": t, "createdAt": 1}),
            )
            .unwrap();
            repo.add_note(M0, &n).unwrap();
        }
    });
}

fn md_files(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".md"))
        .collect();
    v.sort();
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standalone_notes_migration_links_conflicts_and_confinement() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    seed_notes(&data);
    let a = App::launch(&data, Opts::default()).await;

    // First open: the default folder (headless: <data>/notes), the old notes migrated after a backup.
    let info = a.note("vault:get", Value::Null).await;
    let root = std::path::PathBuf::from(info["path"].as_str().unwrap());
    assert!(root.ends_with("notes"), "{info}");
    assert_eq!(info["isDefault"], true);
    let backup: Value =
        serde_json::from_slice(&std::fs::read(data.join("backups/notes-before-vault.json")).unwrap())
            .unwrap();
    assert_eq!(backup[0]["notes"].as_array().unwrap().len(), 2, "{backup}");
    let linked = a.note("doc:list", json!({"meetingId": M0})).await;
    assert_eq!(linked.as_array().unwrap().len(), 1, "{linked}");
    let mig = a.note("doc:get", json!({"id": linked[0]["id"]})).await;
    assert!(
        mig["body"]
            .as_str()
            .unwrap()
            .contains("- [1:01] ask about pricing\n- [2:02] send the deck"),
        "{mig}"
    );
    assert!(a
        .note("doc:list", json!({"meetingId": M1}))
        .await
        .as_array()
        .unwrap()
        .is_empty());
    // The old rows are untouched.
    assert_eq!(
        a.note("note:list-notes", json!({"meetingId": M0}))
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // A standalone note: no meeting needed.
    let d = a.note("doc:create", json!({})).await;
    assert!(d["meetings"].as_array().unwrap().is_empty());
    let d = a
        .note(
            "doc:save",
            json!({"id": d["id"], "body": "Interview prep\nask about the team\n", "baseHash": d["hash"]}),
        )
        .await;
    assert_eq!(d["path"], "Interview prep.md");
    assert_eq!(
        std::fs::read_to_string(root.join("Interview prep.md")).unwrap(),
        format!(
            "---\nid: {}\n---\nInterview prep\nask about the team\n",
            d["id"].as_str().unwrap()
        )
    );
    let found = a.note("doc:list", json!({"query": "team"})).await;
    assert_eq!(found[0]["id"], d["id"]);

    // An outside edit: a save from the stale editor is refused (no silent overwrite), a reload sees it, and the
    // poll watcher reports it.
    std::fs::write(
        root.join("Interview prep.md"),
        format!(
            "---\nid: {}\n---\nInterview prep\nedited in iA Writer\n",
            d["id"].as_str().unwrap()
        ),
    )
    .unwrap();
    let r = a
        .raw(
            "note",
            "doc:save",
            json!({"id": d["id"], "body": "mine", "baseHash": d["hash"]}),
        )
        .await;
    assert_eq!(r["ok"], false, "{r}");
    assert_eq!(r["error"]["details"]["conflict"], true, "{r}");
    let fresh = a.note("doc:get", json!({"id": d["id"]})).await;
    assert!(fresh["body"].as_str().unwrap().contains("edited in iA Writer"));

    // Link and unlink a meeting.
    let l = a
        .note("doc:link-meeting", json!({"id": d["id"], "meetingId": M1}))
        .await;
    assert_eq!(l["meetings"], json!([M1]));
    assert_eq!(
        a.note("doc:list", json!({"meetingId": M1})).await[0]["id"],
        d["id"]
    );
    a.note("doc:unlink-meeting", json!({"id": d["id"], "meetingId": M1}))
        .await;

    // Path-ish ids never reach the file system.
    let r = a.raw("note", "doc:get", json!({"id": "../../etc/passwd"})).await;
    assert_eq!(r["ok"], false);

    // Confinement: another module's view can't reach nano's notes.
    let r = a.raw("echo", "doc:list", json!({})).await;
    assert_eq!(r["ok"], false, "{r}");
    let r = a.raw("ctrl", "vault:get", Value::Null).await;
    assert_eq!(r["ok"], false, "{r}");
    // M1: the shell holds note.read (meetings, transcripts) but not doc.read, so the note files stay nano's.
    for (ch, p) in [
        ("vault:get", Value::Null),
        ("doc:list", json!({})),
        ("doc:get", json!({"id": d["id"]})),
    ] {
        let r = a.raw("shell", ch, p).await;
        assert_eq!(r["ok"], false, "shell {ch}: {r}");
    }

    // Trash = the hidden .trash folder.
    a.note("doc:trash", json!({"id": d["id"]})).await;
    assert!(root.join(".trash/Interview prep.md").exists());
    assert_eq!(md_files(&root).len(), 1);

    // Rebuild from the files.
    assert_eq!(a.note("doc:rebuild-index", Value::Null).await["count"], 1);
    a.quit().await;

    // Restart: no second migration, no second backup write.
    let before = std::fs::read(data.join("backups/notes-before-vault.json")).unwrap();
    let a = App::launch(&data, Opts::default()).await;
    assert_eq!(a.note("doc:list", json!({})).await.as_array().unwrap().len(), 1);
    assert_eq!(
        std::fs::read(data.join("backups/notes-before-vault.json")).unwrap(),
        before
    );
    assert_eq!(md_files(&root).len(), 1);
    a.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mini_hud_quick_add_lands_in_the_meetings_note() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let a = App::launch(&data, Opts::default().standup("0.3")).await;
    a.helper_ready().await;
    a.until_phase("detected").await;
    let s = a.note("note:start", json!({})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();
    a.until_phase("recording").await;
    a.note("note:add-note", json!({"text": "ship it"})).await;
    a.note("note:add-note", json!({"text": "test it"})).await;
    let docs = a.note("doc:list", json!({"meetingId": id})).await;
    assert_eq!(docs.as_array().unwrap().len(), 1, "{docs}");
    let d = a.note("doc:get", json!({"id": docs[0]["id"]})).await;
    assert!(
        d["body"].as_str().unwrap().ends_with("- ship it\n- test it\n"),
        "{d}"
    );
    a.note("note:stop", Value::Null).await;
    a.quit().await;
    // The meeting's old-style rows (the HUD stores there too) don't migrate into a second note on restart.
    let a = App::launch(&data, Opts::default()).await;
    assert_eq!(
        a.note("doc:list", json!({"meetingId": id}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    a.quit().await;
}

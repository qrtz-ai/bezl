//! Phase 5 port of e2e/flows.spec.ts (docs/e2e-parity.md rows 6-10): delete + undo + copy Markdown, settings and
//! the API key across restarts, the HUD (backend half), backup progress on a large recording, and a quit inside
//! the undo window. The 500 meetings are seeded in the test (`dev.seedMany` has no Rust port).
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};
use std::sync::Arc;

fn tombstones(data: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(data.join(qrtz_storage::PENDING_DELETES_FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
        .map(|v| {
            v.iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| x["id"].as_str().unwrap_or("").to_owned())
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn find(a: &App, title: &str) -> Option<String> {
    let list = a.note("note:list", json!({"limit": 1000})).await;
    list.as_array()?
        .iter()
        .find(|m| m["meeting"]["title"] == title)
        .map(|m| m["meeting"]["id"].as_str().unwrap().to_owned())
}

async fn first_title(a: &App) -> String {
    a.note("note:list", json!({})).await[0]["meeting"]["title"]
        .as_str()
        .unwrap_or("")
        .to_owned()
}

/// flows: copy a meeting's Markdown; a deferred delete hides it at once and Undo restores it; a delete that runs
/// out its window is gone from the database, also after a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_a_meeting_with_undo_and_copy_its_markdown() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    seed_many(&data, 500);
    let storage = Arc::new(RecStorage::default());
    let a = App::launch_with(&data, Opts::default(), storage.clone()).await;
    assert_eq!(first_title(&a).await, "Sync #500");
    let id = find(&a, "Sync #500").await.unwrap();
    a.note("note:export", json!({"id": id, "save": false})).await;
    let clip = storage.clip().unwrap();
    assert!(
        clip.starts_with("# Sync #500") || clip.starts_with("# Sync \\#500"),
        "{clip}"
    );

    a.note("note:delete", json!({"id": id, "deferMs": 60_000})).await;
    assert!(find(&a, "Sync #500").await.is_none());
    assert_eq!(first_title(&a).await, "Sync #499");
    assert_eq!(
        a.note("note:undo-delete", json!({"id": id})).await,
        json!({"restored": true})
    );
    assert_eq!(first_title(&a).await, "Sync #500");
    assert_eq!(
        a.note("note:get", json!({"id": id})).await["meeting"]["title"],
        "Sync #500"
    );

    // Delete for real: once the (short) undo window passes it is gone from the database.
    a.note("note:delete", json!({"id": id, "deferMs": 200})).await;
    wait("tombstone completed", || async {
        let gone = a.raw("note", "note:get", json!({"id": id})).await["ok"] == false;
        (gone && tombstones(&a.data).is_empty()).then_some(())
    })
    .await;
    assert_eq!(first_title(&a).await, "Sync #499");
    a.quit().await;
    let b = App::launch(&data, Opts::default()).await;
    assert_eq!(first_title(&b).await, "Sync #499");
    assert!(find(&b, "Sync #500").await.is_none());
    b.quit().await;
}

/// flows: settings persist across restarts; the API key is stored only in the owner-only secret store (never in
/// the config or anywhere else under the data dir, never sent back); removing it returns to the extractive path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_persist_across_restarts_and_the_api_key_is_saved_and_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let a = App::launch(&data, Opts::default()).await;
    a.settings(
        "config:set",
        json!({"patch": {"note": {"retainAudioDays": 90, "defaultTemplate": "standup"}, "theme": "light"}}),
    )
    .await;
    a.settings(
        "secrets:set",
        json!({"name": "llm.openrouter", "value": "sk-test-not-real"}),
    )
    .await;
    assert!(a
        .settings("secrets:list", Value::Null)
        .await
        .as_array()
        .unwrap()
        .contains(&json!("llm.openrouter")));
    a.quit().await;

    let b = App::launch(&data, Opts::default()).await;
    let cfg = b.settings("config:get", Value::Null).await;
    assert_eq!(cfg["theme"], "light", "{cfg}");
    assert_eq!(cfg["note"]["retainAudioDays"], 90, "{cfg}");
    assert_eq!(cfg["note"]["defaultTemplate"], "standup", "{cfg}");
    // "Key configured": the name is listed, the value never comes back over IPC.
    let names = b.settings("secrets:list", Value::Null).await;
    assert_eq!(names, json!(["llm.openrouter"]));
    assert!(!cfg.to_string().contains("sk-test-not-real"));
    // On disk only in the 0600 secret store (Linux file store; Keychain on macOS is Mac-only).
    for e in walk(&data) {
        let txt = std::fs::read(&e).unwrap_or_default();
        let has = String::from_utf8_lossy(&txt).contains("sk-test-not-real");
        if e.file_name().is_some_and(|n| n == "secrets.json") {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(&e).unwrap().permissions().mode() & 0o777, 0o600);
            }
        } else {
            assert!(!has, "key leaked into {}", e.display());
        }
    }
    // Remove → "Using extractive fallback".
    b.settings("secrets:set", json!({"name": "llm.openrouter", "value": null}))
        .await;
    assert_eq!(b.settings("secrets:list", Value::Null).await, json!([]));
    b.quit().await;
}

fn walk(d: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(d).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else if std::fs::metadata(&p)
            .map(|m| m.len() < 5_000_000)
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
    out
}

/// flows: the mini HUD (backend half: the window, its theme and axe are the renderer's): recording asks the shell
/// for the HUD and the theme it follows is the stored one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mini_hud_backend_half_expands_and_follows_the_light_theme() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("data"), Opts::default().standup("0.04")).await;
    a.settings("config:set", json!({"patch": {"theme": "light"}}))
        .await;
    a.helper_ready().await;
    a.until_phase("detected").await;
    a.note("note:start", json!({})).await;
    assert_eq!(a.call("shell", "config:get", Value::Null).await["theme"], "light");
    assert_eq!(
        a.call("shell", "hud:set-expanded", json!({"expanded": true}))
            .await,
        json!({"expanded": true})
    );
    assert_eq!(*a.shell.hud.lock().unwrap(), vec![true]);
    a.note("note:stop", Value::Null).await;
    a.until_phase("done").await;
    a.quit().await;
}

/// flows: a ~400 MB recording shows backup progress (checksums, then writing the zip) and finishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_progress_on_a_large_recording() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    // A finished meeting that owns the large recording.
    let a = App::launch(&data, Opts::default()).await;
    a.helper_ready().await;
    let (id, _) = record_until(&a, Some("Big"), |t| segments(t) > 0).await;
    let wav = a.note("note:get", json!({"id": id})).await["meeting"]["audioPath"]
        .as_str()
        .map(std::path::PathBuf::from);
    a.quit().await;
    let wav = wav.unwrap_or_else(|| data.join("recordings").join(format!("{id}.wav")));
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&wav).unwrap();
        let mut h = Vec::from(&b"RIFF"[..]);
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(b"WAVEfmt ");
        f.write_all(&h).unwrap();
        let chunk = vec![5u8; 8 * 1024 * 1024];
        for _ in 0..48 {
            f.write_all(&chunk).unwrap();
        }
    }
    let out = tmp.path().join("out").join("backup.zip");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let storage = Arc::new(RecStorage::default());
    *storage.backup.lock().unwrap() = Some(out.clone());
    let b = App::launch_with(&data, Opts::default(), storage).await;
    let mut ev = b.app.service.events.subscribe();
    let rep = b.settings("data:export-backup", Value::Null).await;
    assert_eq!(rep["meetings"], 1, "{rep}");
    assert_eq!(rep["audioFiles"], 1, "{rep}");
    let mut phases = vec![];
    while let Ok((name, p)) = ev.try_recv() {
        if name == "data:backup-progress" {
            phases.push(p["phase"].as_str().unwrap_or("").to_owned());
        }
    }
    assert!(phases.iter().any(|p| p == "write"), "{phases:?}");
    assert!(phases.len() > 2, "progress is reported along the way: {phases:?}");
    assert_eq!(phases.last().map(String::as_str), Some("done"), "{phases:?}");
    assert!(std::fs::metadata(&out).unwrap().len() > 400_000_000);
    b.quit().await;
}

/// flows: quitting inside the undo window still deletes; the tombstone is completed on the next launch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quitting_inside_the_undo_window_still_deletes_on_next_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    seed_many(&data, 500);
    let a = App::launch(&data, Opts::default()).await;
    let id = find(&a, "Sync #500").await.unwrap();
    a.note("note:delete", json!({"id": id, "deferMs": 6000})).await;
    assert_eq!(tombstones(&data), vec![id.clone()]);
    a.quit().await; // well inside the 6 s window
    assert_eq!(tombstones(&data), vec![id.clone()]);

    let b = App::launch(&data, Opts::default()).await;
    wait("tombstone completed at launch", || async {
        tombstones(&data).is_empty().then_some(())
    })
    .await;
    assert_eq!(first_title(&b).await, "Sync #499");
    assert!(find(&b, "Sync #500").await.is_none());
    b.quit().await;
}

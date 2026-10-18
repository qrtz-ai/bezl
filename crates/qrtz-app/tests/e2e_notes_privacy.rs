//! Phase 5 port of e2e/notes-privacy.spec.ts (docs/e2e-parity.md rows 22–24): live notes + privacy pause, notes in
//! the summary, note edit, template re-run; the privacy config persists (local-only went with ADR 0076: qrtz is
//! entirely offline), an excluded app is never detected; backup export with checksums, import into a fresh profile,
//! re-import skips, a tampered zip is rejected.
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, OnceLock};

/// The process-wide log, captured from `tracing` (the Tauri build logs through `tracing`; Electron's
/// `logs/main.jsonl` has no Rust equivalent).
fn log() -> &'static Arc<Mutex<Vec<u8>>> {
    static L: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    L.get_or_init(|| {
        #[derive(Clone)]
        struct W(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for W {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = Arc::new(Mutex::new(Vec::new()));
        let w = W(buf.clone());
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(move || w.clone())
            .try_init();
        buf
    })
}

fn logged(needle: &str) -> bool {
    String::from_utf8_lossy(&log().lock().unwrap()).contains(needle)
}

fn config(data: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(data.join("config.json")).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_notes_and_privacy_pause_notes_merged_into_the_summary_and_templates_re_run() {
    let tmp = tempfile::tempdir().unwrap();
    // ADR 0075: the summary is written on-device; the notes reach the model as the user's priorities.
    let plog = tmp.path().join("summarize.jsonl");
    let last_prompt = || {
        let l = std::fs::read_to_string(&plog).unwrap_or_default();
        let v: Value = serde_json::from_str(l.lines().last().unwrap_or("{}")).unwrap();
        v["prompt"].as_str().unwrap_or_default().to_owned()
    };
    let a = App::launch(
        &tmp.path().join("data"),
        Opts::default()
            .standup("0.3")
            .env("QRTZ_MOCK_SUMMARIZE_LOG", plog.to_str().unwrap()),
    )
    .await;
    a.helper_ready().await;
    a.until_phase("detected").await;
    let s = a.note("note:start", json!({})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();
    a.until_phase("recording").await;

    // Pause straight away: nothing is transcribed while paused.
    assert_eq!(
        a.note("note:pause", json!({"paused": true})).await["paused"],
        true
    );
    let n1 = a
        .note(
            "note:add-note",
            json!({"text": "Ask Grace about the launch checklist"}),
        )
        .await;
    assert_eq!(n1["meetingId"], json!(id));
    assert!(n1["atMs"].as_f64().unwrap() < 10_000.0, "stamped 0:0x: {n1}");
    // Let the recording clock run past the first two scripted lines (2.94 s at speed 0.3) while paused.
    a.until_session("3 s recorded while paused", |s| {
        s["elapsedMs"].as_u64().unwrap_or(0) > 3_500
    })
    .await;
    let t = a.note("note:live-transcript", json!({})).await;
    assert_eq!(segments(&t), 0, "transcribed while paused: {t}");
    assert_eq!(a.session().await["paused"], true);
    assert_eq!(
        a.note("note:pause", json!({"paused": false})).await["paused"],
        false
    );
    let n2 = a
        .note("note:add-note", json!({"text": "TODO send recap to the team"}))
        .await;
    assert_eq!(
        a.note("note:list-notes", json!({"meetingId": id}))
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
    wait("lines after resume", || async {
        let t = a.note("note:live-transcript", json!({})).await;
        (segments(&t) > 1).then_some(())
    })
    .await;
    a.note("note:stop", Value::Null).await;
    a.until_phase("done").await;
    a.until_settled(&id).await;

    let d = a.note("note:get", json!({"id": id})).await;
    assert_eq!(d["summary"]["provider"], "apple-intelligence", "{d}");
    let p = last_prompt();
    let notes_at = p.find("My notes").expect("notes in the prompt");
    let fence_at = p.rfind("<transcript-").expect("fenced transcript");
    assert!(notes_at < fence_at, "notes are outside the transcript fence: {p}");
    for n in [
        "Ask Grace about the launch checklist",
        "TODO send recap to the team",
    ] {
        assert!(p[notes_at..fence_at].contains(n), "{n} in {p}");
    }

    // Edit a note; the change is saved.
    a.note(
        "note:update-note",
        json!({"id": n2["id"], "text": "TODO send recap to everyone"}),
    )
    .await;
    let notes = a.note("note:list-notes", json!({"meetingId": id})).await;
    assert!(
        notes.to_string().contains("TODO send recap to everyone"),
        "{notes}"
    );

    // Template: "general" by default; "standup" re-runs the summary and is stored with the meeting.
    assert_eq!(meeting_template(&d), "general", "{d}");
    let before = a.until_settled(&id).await;
    let r = a
        .note("note:set-template", json!({"id": id, "template": "standup"}))
        .await;
    assert_eq!(r["provider"], "apple-intelligence", "{r}");
    assert_eq!(r["template"], "standup", "{r}");
    // N115: through the pipeline's summary step (one heavy step at a time), like resummarize.
    let after = a.until_settled(&id).await;
    assert_ne!(
        after["steps"]["summary"]["finishedAt"], before["steps"]["summary"]["finishedAt"],
        "{after}"
    );
    let p = last_prompt();
    assert!(p.starts_with("This is a standup."), "{p}");
    assert!(p.contains("TODO send recap to everyone"), "{p}");
    a.quit().await;
    let b = App::launch(&tmp.path().join("data"), Opts::default()).await;
    let d = b.note("note:get", json!({"id": id})).await;
    assert_eq!(meeting_template(&d), "standup", "{d}");
    assert_eq!(d["summary"]["template"], "standup", "{d}");
    b.quit().await;
}

fn meeting_template(d: &Value) -> String {
    d["meeting"]["template"]
        .as_str()
        .or(d["template"].as_str())
        .unwrap_or("general")
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn privacy_config_persists_and_excluded_apps_are_not_detected() {
    log();
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let a = App::launch(&data, Opts::default().standup("0.02")).await;
    a.helper_ready().await;
    a.settings(
        "config:set",
        json!({"patch": {"privacy": {"pauseHotkey": "CommandOrControl+Alt+9",
                                      "excludedApps": ["Safari", "webex"]}}}),
    )
    .await;
    // ADR 0146 M2.3: privacy is nano's, stored in nano's own settings file.
    let nano: Value =
        serde_json::from_str(&std::fs::read_to_string(data.join("modules/nano/settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        nano["privacy"],
        json!({"excludedApps": ["Safari", "webex"], "pauseHotkey": "CommandOrControl+Alt+9"})
    );
    assert!(config(&data).get("privacy").is_none());
    a.until_phase("detected").await; // zoom is not excluded
    let (id, _) = record_until(&a, None, |t| segments(t) > 1).await;
    assert!(!id.is_empty());
    a.quit().await;

    // Excluded app: the Zoom meeting is never detected (no prompt); the log says why.
    let ud = tmp.path().join("excluded");
    std::fs::create_dir_all(&ud).unwrap();
    std::fs::write(
        ud.join("config.json"),
        json!({"privacy": {"excludedApps": ["zoom"]}}).to_string(),
    )
    .unwrap();
    let b = App::launch(&ud, Opts::default().standup("0.02")).await;
    b.helper_ready().await;
    wait("meeting ignored (excluded app)", || async {
        logged("meeting ignored (excluded app)").then_some(())
    })
    .await;
    let s = b.session().await;
    assert_eq!(s["phase"], "idle", "{s}");
    assert!(s["detected"].is_null(), "{s}");
    b.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_export_with_checksums_import_into_a_fresh_profile_and_a_tampered_zip_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("qrtz-backup.zip");
    let storage = Arc::new(RecStorage::default());
    *storage.backup.lock().unwrap() = Some(zip.clone());
    let a = App::launch_with(
        &tmp.path().join("a"),
        Opts::default().standup("0.02"),
        storage.clone(),
    )
    .await;
    a.helper_ready().await;
    a.until_phase("detected").await;
    let s = a.note("note:start", json!({})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();
    a.until_phase("recording").await;
    a.note("note:add-note", json!({"text": "backup me"})).await;
    wait("two lines", || async {
        (segments(&a.note("note:live-transcript", json!({})).await) > 1).then_some(())
    })
    .await;
    a.note("note:stop", Value::Null).await;
    a.until_phase("done").await;
    a.until_settled(&id).await;
    let rep = a.settings("data:export-backup", Value::Null).await;
    assert_eq!(
        (rep["meetings"].clone(), rep["audioFiles"].clone()),
        (json!(1), json!(1)),
        "{rep}"
    );
    assert!(std::fs::metadata(&zip).unwrap().len() > 10_000);
    a.quit().await;

    // A fresh profile: verified and imported 1 meeting with its recording; a second import skips it.
    let bdir = tmp.path().join("b");
    let b = App::launch_with(&bdir, Opts::default(), storage.clone()).await;
    let r = b.settings("data:import-backup", Value::Null).await;
    assert_eq!(
        (
            r["imported"].clone(),
            r["skipped"].clone(),
            r["audioFiles"].clone()
        ),
        (json!(1), json!(0), json!(1)),
        "{r}"
    );
    let r = b.settings("data:import-backup", Value::Null).await;
    assert_eq!(
        (r["imported"].clone(), r["skipped"].clone()),
        (json!(0), json!(1)),
        "{r}"
    );
    let d = b.note("note:get", json!({"id": id})).await;
    // The summary (on-device, ADR 0075) and the note came across with the meeting.
    assert_eq!(d["summary"]["provider"], "apple-intelligence", "{d}");
    assert!(d["notes"].to_string().contains("backup me"), "{d}");
    assert_eq!(std::fs::read_dir(bdir.join("recordings")).unwrap().count(), 1);
    b.quit().await;

    // One flipped byte inside the stored audio: the integrity check rejects the whole backup.
    let mut buf = std::fs::read(&zip).unwrap();
    let i = buf.windows(4).position(|w| w == b"data").unwrap() + 2000;
    buf[i] ^= 0xff;
    let tampered = tmp.path().join("tampered.zip");
    std::fs::write(&tampered, &buf).unwrap();
    *storage.backup.lock().unwrap() = Some(tampered);
    let cdir = tmp.path().join("c");
    let c = App::launch_with(&cdir, Opts::default(), storage).await;
    let r = c.raw("settings", "data:import-backup", Value::Null).await;
    assert_eq!(r["ok"], false, "tampered backup accepted: {r}");
    assert_eq!(
        c.note("note:list", json!({})).await,
        json!([]),
        "nothing imported"
    );
    c.quit().await;
}

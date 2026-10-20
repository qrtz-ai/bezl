#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Import semantics beyond the format: duplicate ids, audio name collisions, tombstones, disk full, size caps.
mod common;

use std::path::Path;

use common::*;
use qrtz_backup::{export_backup, no_progress, ExportBackup, ImportLimits, Phase};
use qrtz_storage::model::{Meeting, Segment, SegmentSource};
use qrtz_storage::{PendingDeletes, StorageError};
use serde_json::json;

fn meeting(id: &str, title: &str, audio: Option<&Path>) -> Meeting {
    serde_json::from_value(json!({
        "id": id, "title": title, "platform": "zoom", "startedAt": 1_760_000_000_000i64, "status": "done",
        "participants": [], "audioPath": audio.map(|p| p.to_string_lossy().into_owned()),
    }))
    .unwrap()
}

fn seg(id: &str, i: i64, text: &str) -> Segment {
    serde_json::from_value(
        json!({"id": id, "startMs": i * 1000, "endMs": i * 1000 + 900, "text": text, "source": "live"}),
    )
    .unwrap()
}

/// A library with meetings `ids`, each with `lines` segments and a recording `<id>.wav` of `wav_bytes` bytes.
fn library(ids: &[&str], lines: usize, wav_bytes: usize) -> Lib {
    let lib = Lib::new();
    std::fs::create_dir_all(lib.rec()).unwrap();
    for (k, id) in ids.iter().enumerate() {
        let wav = lib.rec().join(format!("{id}.wav"));
        std::fs::write(&wav, vec![k as u8 + 1; wav_bytes]).unwrap();
        let mut m = meeting(id, &format!("Meeting {id}"), Some(&wav));
        m.started_at += k as i64;
        lib.db.notes().upsert_meeting(&m).unwrap();
        let segs: Vec<_> = (0..lines)
            .map(|i| {
                seg(
                    &format!("{id}-{i}"),
                    i as i64,
                    &format!("line {i} of {id}: the roadmap and the hiring plan"),
                )
            })
            .collect();
        lib.db
            .notes()
            .replace_segments(id, SegmentSource::Live, &segs)
            .unwrap();
    }
    lib
}

fn backup_of(lib: &Lib) -> std::path::PathBuf {
    let out = lib.dir.path().join("b.zip");
    export_backup(
        &ExportBackup {
            db: &lib.db,
            target: &out,
            recordings_dir: &lib.rec(),
            temp_dir: &lib.tmp(),
            app_version: "t",
            now_ms: 1,
        },
        &mut no_progress,
    )
    .unwrap();
    out
}

#[test]
fn duplicate_ids_are_skipped_untouched_and_their_audio_is_not_restored() {
    let src = library(&["a", "b"], 3, 100);
    let zip = backup_of(&src);
    let dst = Lib::new();
    std::fs::create_dir_all(dst.rec()).unwrap();
    // `a` already exists here with different content and a different recording of the same name.
    let mine = dst.rec().join("a.wav");
    std::fs::write(&mine, b"my own recording").unwrap();
    dst.db
        .notes()
        .upsert_meeting(&meeting("a", "Mine", Some(&mine)))
        .unwrap();
    let r = dst.import(&zip).unwrap();
    assert_eq!((r.imported, r.skipped, r.audio_files), (1, 1, 1));
    let a = dst.db.notes().get_meeting("a").unwrap().unwrap();
    assert_eq!(a.title, "Mine");
    assert_eq!(std::fs::read(&mine).unwrap(), b"my own recording");
    assert_eq!(
        dst.rec_files(),
        ["a.wav", "b.wav"],
        "no -restored copy for a skipped meeting"
    );
    assert_eq!(dst.db.notes().get_segments("b").unwrap().len(), 3);
}

#[test]
fn an_imported_recording_whose_name_is_taken_lands_under_a_restored_name() {
    let src = library(&["a"], 1, 100);
    let zip = backup_of(&src);
    let dst = Lib::new();
    std::fs::create_dir_all(dst.rec()).unwrap();
    std::fs::write(dst.rec().join("a.wav"), b"unrelated file").unwrap();
    let r = dst.import(&zip).unwrap();
    assert_eq!((r.imported, r.audio_files), (1, 1));
    let p = dst
        .db
        .notes()
        .get_meeting("a")
        .unwrap()
        .unwrap()
        .audio_path
        .unwrap();
    let name = Path::new(&p).file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("a-restored-") && name.ends_with(".wav"),
        "{name}"
    );
    assert_eq!(Path::new(&p).parent().unwrap(), dst.rec());
    assert_eq!(std::fs::read(&p).unwrap(), vec![1u8; 100]);
    assert_eq!(std::fs::read(dst.rec().join("a.wav")).unwrap(), b"unrelated file");
    // An identical file already in place is reused, not copied.
    let again = Lib::new();
    std::fs::create_dir_all(again.rec()).unwrap();
    std::fs::write(again.rec().join("a.wav"), vec![1u8; 100]).unwrap();
    again.import(&zip).unwrap();
    assert_eq!(again.rec_files(), ["a.wav"]);
}

#[test]
fn recordings_dir_is_tightened_to_owner_only() {
    let src = library(&["a"], 1, 10);
    let zip = backup_of(&src);
    let dst = Lib::new();
    std::fs::create_dir_all(dst.rec()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dst.rec(), std::fs::Permissions::from_mode(0o755)).unwrap();
        dst.import(&zip).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dst.rec()), 0o700);
        assert_eq!(mode(&dst.rec().join("a.wav")), 0o600);
    }
}

#[test]
fn tombstones_a_pending_delete_is_skipped_and_a_stale_one_is_cleared() {
    let src = library(&["live", "gone"], 2, 10);
    let zip = backup_of(&src);
    let dst = Lib::new();
    let pd = PendingDeletes::new(dst.dir.path());
    // `live` is still here, inside its undo window: the import skips it and the delete stays scheduled.
    dst.db
        .notes()
        .upsert_meeting(&meeting("live", "Live", None))
        .unwrap();
    pd.add("live").unwrap();
    // `gone` was deleted, but its tombstone outlived the delete: importing must not leave it armed.
    pd.add("gone").unwrap();
    let r = dst.import_with(&zip, Some(&pd), ImportLimits::default()).unwrap();
    assert_eq!((r.imported, r.skipped), (1, 1));
    assert_eq!(pd.ids(), ["live"]);
    assert!(dst.db.notes().get_meeting("gone").unwrap().is_some());
    assert_eq!(dst.db.notes().get_meeting("live").unwrap().unwrap().title, "Live");
}

#[test]
fn disk_full_while_staging_leaves_nothing() {
    let src = library(&["a", "b", "c"], 5, 50_000);
    let zip = backup_of(&src);
    let dst = Lib::new();
    let lim = ImportLimits {
        fail_write_after: Some(80_000),
        ..ImportLimits::default()
    };
    let err = dst.import_with(&zip, None, lim).unwrap_err();
    assert!(
        matches!(&err, StorageError::Io(e) if e.raw_os_error() == Some(28)),
        "{err}"
    );
    assert!(dst.db.notes().list_meetings(10, 0).unwrap().is_empty());
    assert!(dst.rec_files().is_empty(), "{:?}", dst.rec_files());
    assert!(dst.tmp_files().is_empty(), "{:?}", dst.tmp_files());
    let r = dst.import(&zip).unwrap();
    assert_eq!(r.imported, 3);
}

#[test]
fn disk_full_in_the_database_mid_meeting_leaves_no_partial_meeting_and_reimport_repairs() {
    let src = library(&["a", "b", "c"], 1_500, 1_000);
    let zip = backup_of(&src);
    let dst = Lib::new();
    let pages: i64 = dst
        .db
        .connection()
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    // Room for about one meeting's transcript, not three.
    dst.db
        .connection()
        .execute_batch(&format!("PRAGMA max_page_count = {}", pages + 60))
        .unwrap();
    let err = dst.import(&zip).unwrap_err();
    assert!(err.to_string().contains("full"), "{err}");
    let n = dst.db.notes();
    let present = n.list_meetings(10, 0).unwrap();
    assert!(present.len() < 3);
    for m in &present {
        // Whatever landed landed whole: transcript and recording.
        assert_eq!(n.get_segments(&m.id).unwrap().len(), 1_500, "{}", m.id);
        assert!(Path::new(m.audio_path.as_ref().unwrap()).exists());
    }
    let mut want: Vec<String> = present.iter().map(|m| format!("{}.wav", m.id)).collect();
    want.sort();
    assert_eq!(
        dst.rec_files(),
        want,
        "the failed meeting's recording was taken back"
    );
    assert!(dst.tmp_files().is_empty());
    dst.db
        .connection()
        .execute_batch("PRAGMA max_page_count = 1073741823")
        .unwrap();
    let r = dst.import(&zip).unwrap();
    assert_eq!(r.imported as usize + present.len(), 3);
    for id in ["a", "b", "c"] {
        assert_eq!(n.get_segments(id).unwrap().len(), 1_500);
    }
    assert_eq!(dst.rec_files(), ["a.wav", "b.wav", "c.wav"]);
}

#[test]
fn size_caps_refuse_bombs_before_writing() {
    let src = library(&["a"], 1, 4 << 20);
    let zip = backup_of(&src);
    for (lim, what) in [
        (
            ImportLimits {
                max_total_bytes: 1 << 20,
                ..Default::default()
            },
            "larger than the import limit",
        ),
        (
            ImportLimits {
                max_entry_bytes: 1 << 20,
                ..Default::default()
            },
            "larger than the import limit",
        ),
    ] {
        let dst = Lib::new();
        let err = dst.import_with(&zip, None, lim).unwrap_err();
        assert!(err.to_string().contains(what), "{err}");
        assert!(dst.rec_files().is_empty() && dst.tmp_files().is_empty());
    }
    // An honest-size entry that inflates more than 1000x is refused by ratio.
    let dir = tempfile::tempdir().unwrap();
    let bomb = dir.path().join("bomb.zip");
    let zeros = vec![0u8; 32 << 20];
    let db = std::fs::read(zip_db(&zip, dir.path())).unwrap();
    let m = json!({"format": "qrtz-backup", "version": 1, "createdAt": 1, "appVersion": "t", "schemaVersion": 4,
        "meetings": 0, "files": [
            {"path": "db/qrtz.db", "sha256": sha(&db), "bytes": db.len()},
            {"path": "audio/z.wav", "sha256": sha(&zeros), "bytes": zeros.len()}]});
    {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        let mut z = zip::ZipWriter::new(std::fs::File::create(&bomb).unwrap());
        let d = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("db/qrtz.db", d).unwrap();
        z.write_all(&db).unwrap();
        z.start_file("audio/z.wav", d).unwrap();
        z.write_all(&zeros).unwrap();
        z.start_file("manifest.json", d).unwrap();
        z.write_all(m.to_string().as_bytes()).unwrap();
        z.finish().unwrap();
    }
    let dst = Lib::new();
    let err = dst.import(&bomb).unwrap_err();
    assert!(
        err.to_string()
            .contains("audio/z.wav is larger than the import limit"),
        "{err}"
    );
    assert!(dst.rec_files().is_empty() && dst.tmp_files().is_empty());
}

fn zip_db(zip: &Path, dir: &Path) -> std::path::PathBuf {
    let mut z = zip::ZipArchive::new(std::fs::File::open(zip).unwrap()).unwrap();
    let out = dir.join("db.bin");
    std::io::copy(
        &mut z.by_name("db/qrtz.db").unwrap(),
        &mut std::fs::File::create(&out).unwrap(),
    )
    .unwrap();
    out
}

#[test]
fn duplicate_zip_entries_are_refused() {
    let src = library(&["a", "b"], 1, 10);
    let zip = backup_of(&src);
    // Rename audio/b.wav to audio/a.wav in the headers: two entries, one name.
    let mut bytes = std::fs::read(&zip).unwrap();
    let (from, to) = (b"audio/b.wav", b"audio/a.wav");
    let mut i = 0;
    while i + from.len() <= bytes.len() {
        if &bytes[i..i + from.len()] == from {
            bytes[i..i + from.len()].copy_from_slice(to);
        }
        i += 1;
    }
    let forged = src.dir.path().join("dup.zip");
    std::fs::write(&forged, bytes).unwrap();
    let dst = Lib::new();
    let err = dst.import(&forged).unwrap_err();
    assert!(err.to_string().starts_with("Backup rejected:"), "{err}");
    assert!(dst.rec_files().is_empty() && dst.tmp_files().is_empty());
}

#[test]
fn export_progress_is_monotonic_and_skips_unsafe_recording_names() {
    let src = library(&["a"], 1, 300_000);
    std::fs::write(src.rec().join("bad name.wav"), b"x").unwrap();
    std::fs::write(src.rec().join("notes.txt"), b"x").unwrap();
    let out = src.dir.path().join("p.zip");
    let mut seen: Vec<(Phase, u64, u64)> = Vec::new();
    let r = export_backup(
        &ExportBackup {
            db: &src.db,
            target: &out,
            recordings_dir: &src.rec(),
            temp_dir: &src.tmp(),
            app_version: "t",
            now_ms: 1,
        },
        &mut |p, d, t| seen.push((p, d, t)),
    )
    .unwrap();
    assert_eq!(r.audio_files, 1);
    assert!(seen.windows(2).all(|w| w[0].1 <= w[1].1));
    let last = seen.last().unwrap();
    assert_eq!(last.1, last.2);
    let z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    assert_eq!(z.file_names().count(), 3);
    // Import progress ends with the import phase.
    let dst = Lib::new();
    let mut phases = Vec::new();
    qrtz_backup::import_backup(
        &qrtz_backup::ImportBackup {
            db: &dst.db,
            source: &out,
            recordings_dir: &dst.rec(),
            temp_dir: &dst.tmp(),
            pending: None,
            limits: ImportLimits::default(),
        },
        &mut |p, _, _| phases.push(p),
    )
    .unwrap();
    assert_eq!(phases.first(), Some(&Phase::Verify));
    assert_eq!(phases.last(), Some(&Phase::Import));
}

#[test]
fn a_failed_export_leaves_no_partial_archive() {
    let src = library(&["a"], 1, 10);
    let out = src.dir.path().join("x.zip");
    std::fs::create_dir(&out).unwrap(); // the rename onto a directory fails
    let err = export_backup(
        &ExportBackup {
            db: &src.db,
            target: &out,
            recordings_dir: &src.rec(),
            temp_dir: &src.tmp(),
            app_version: "t",
            now_ms: 1,
        },
        &mut no_progress,
    );
    assert!(err.is_err());
    assert!(src.tmp_files().is_empty());
    assert!(!src.dir.path().join("x.zip.partial").exists());
}

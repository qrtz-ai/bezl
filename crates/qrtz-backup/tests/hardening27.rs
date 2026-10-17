//! Hardening cycle 27, N132 (ADR 0086): a backup export took every `*.wav` in the recordings dir that `is_file()`,
//! which follows symlinks. A symlink planted under a recording's name (`x.wav` → `~/.ssh/id_ed25519`) put its
//! target in the zip, and a FIFO named `x.wav` would block the export for good. Now only regular files themselves
//! go in; the meeting's own recordings are unaffected and still round-trip.
#![cfg(unix)]
mod common;

use common::*;
use qrtz_backup::{export_backup, no_progress, write_backup, ExportBackup, WriteBackup};

fn export(lib: &Lib) -> (qrtz_backup::ExportReport, Vec<String>, Vec<u8>) {
    let out = lib.dir.path().join("b.zip");
    let r = export_backup(
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
    let z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let mut names: Vec<String> = z.file_names().map(str::to_owned).collect();
    names.sort();
    (r, names, std::fs::read(&out).unwrap())
}

#[test]
fn a_symlink_fifo_or_directory_under_a_recording_name_stays_out_of_the_backup() {
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("id_ed25519");
    std::fs::write(&secret, b"-----BEGIN OPENSSH PRIVATE KEY----- s3cr3t").unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    // The library lives on the export's thread (the Db is not Sync), so a blocked open shows as a timeout here.
    std::thread::spawn(move || {
        let lib = Lib::new();
        std::fs::create_dir_all(lib.rec()).unwrap();
        std::fs::write(lib.rec().join("a.wav"), b"RIFF own recording").unwrap();
        std::os::unix::fs::symlink(&secret, lib.rec().join("b.wav")).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(lib.rec().join("c.wav"))
            .status()
            .unwrap()
            .success());
        std::fs::create_dir(lib.rec().join("d.wav")).unwrap();
        let _ = tx.send(export(&lib));
    });
    let (r, names, bytes) = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the export blocked (a FIFO opened?)");
    assert_eq!(r.audio_files, 1);
    assert!(names.contains(&"audio/a.wav".to_owned()), "{names:?}");
    assert!(
        !names
            .iter()
            .any(|n| n.contains("b.wav") || n.contains("c.wav") || n.contains("d.wav")),
        "{names:?}"
    );
    assert!(
        !bytes.windows(6).any(|w| w == b"s3cr3t"),
        "a symlink's target reached the backup"
    );
}

/// The lower-level writer, handed a list that names a symlink (a list made before the swap), skips it too.
#[test]
fn write_backup_skips_a_listed_path_that_is_a_symlink() {
    let lib = Lib::new();
    std::fs::create_dir_all(lib.rec()).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("s.txt");
    std::fs::write(&secret, b"s3cr3t outside").unwrap();
    let link = lib.rec().join("e.wav");
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    let own = lib.rec().join("f.wav");
    std::fs::write(&own, b"RIFF").unwrap();
    let snap = lib.dir.path().join("snap.db");
    lib.db.snapshot_to(&snap).unwrap();
    let out = lib.dir.path().join("w.zip");
    let r = write_backup(
        &WriteBackup {
            target: &out,
            db_snapshot: &snap,
            audio_files: &[link.clone(), own.clone()],
            app_version: "t",
            schema_version: 1,
            meetings: 0,
            now_ms: 1,
        },
        &mut no_progress,
    )
    .unwrap();
    assert_eq!(r.audio_files, 1);
    let bytes = std::fs::read(&out).unwrap();
    assert!(!bytes.windows(6).any(|w| w == b"s3cr3t"));
}

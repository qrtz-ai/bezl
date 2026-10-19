//! Electron <-> Rust backup parity (spec/fixtures/backup; TS side: apps/desktop/src/main/backup.parity.test.ts).
//!
//! - electron-v1.zip (TS-made) imports here to expected.json.
//! - rust-v1.zip is electron-v1.zip imported here and exported again; the TS test imports it back (E -> R -> E).
//! - malicious/*.zip + cases.json are generated here; both sides must reject every one and leave nothing behind.
//!
//! `QRTZ_FIXTURES_UPDATE=1 cargo test -p qrtz-backup --test parity` rewrites rust-v1.zip and malicious/.
mod common;

use std::io::Write;
use std::path::Path;

use common::*;
use qrtz_backup::{export_backup, no_progress, ExportBackup};
use serde_json::{json, Value};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn export(lib: &Lib, target: &Path, now_ms: i64) -> qrtz_backup::ExportReport {
    export_backup(
        &ExportBackup {
            db: &lib.db,
            target,
            recordings_dir: &lib.rec(),
            temp_dir: &lib.tmp(),
            app_version: "0.1.0-fixture",
            now_ms,
        },
        &mut no_progress,
    )
    .unwrap()
}

#[test]
fn electron_backup_imports_to_the_same_library() {
    let lib = Lib::new();
    let r = lib.import(&fixtures().join("electron-v1.zip")).unwrap();
    assert_eq!((r.imported, r.skipped, r.audio_files), (3, 0, 1));
    assert_eq!(dump(&lib.db), expected());
    // Only the referenced recording is restored (orphan.wav is not), owner-only.
    assert_eq!(lib.rec_files(), ["m-a.wav"]);
    assert!(lib.tmp_files().is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&lib.rec()), 0o700);
        assert_eq!(mode(&lib.rec().join("m-a.wav")), 0o600);
    }
    // Re-import: every id is present, nothing new on disk.
    let again = lib.import(&fixtures().join("electron-v1.zip")).unwrap();
    assert_eq!((again.imported, again.skipped, again.audio_files), (0, 3, 0));
    assert_eq!(lib.rec_files(), ["m-a.wav"]);
}

#[test]
fn rust_export_of_the_electron_backup_round_trips() {
    let lib = Lib::new();
    lib.import(&fixtures().join("electron-v1.zip")).unwrap();
    let out = lib.dir.path().join("out.zip");
    let rep = export(&lib, &out, 1_760_600_000_000);
    assert_eq!((rep.meetings, rep.audio_files), (3, 1));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&out).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(!lib.dir.path().join("out.zip.partial").exists());
    assert!(lib.tmp_files().is_empty(), "snapshot dir removed");
    let rust = fixtures().join("rust-v1.zip");
    if update() {
        std::fs::copy(&out, &rust).unwrap();
    }
    for zip in [&out, &rust] {
        let other = Lib::new();
        let r = other.import(zip).unwrap();
        assert_eq!((r.imported, r.skipped), (3, 0));
        assert_eq!(dump(&other.db), expected());
    }
    // The manifest is the TS shape, in TS key order.
    let mut z = zip::ZipArchive::new(std::fs::File::open(&rust).unwrap()).unwrap();
    let names: Vec<String> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
    assert_eq!(names, ["db/qrtz.db", "audio/m-a.wav", "manifest.json"]);
    let mut raw = String::new();
    std::io::Read::read_to_string(&mut z.by_name("manifest.json").unwrap(), &mut raw).unwrap();
    assert!(raw.starts_with(
        "{\n  \"format\": \"qrtz-backup\",\n  \"version\": 1,\n  \"createdAt\": 1760600000000,"
    ));
    assert_eq!(
        z.by_name("audio/m-a.wav").unwrap().compression(),
        CompressionMethod::Stored
    );
    assert_eq!(
        z.by_name("db/qrtz.db").unwrap().compression(),
        CompressionMethod::Deflated
    );
}

// ------------------------------------------------------------------ malicious / corrupt archives

type Entry<'a> = (&'a str, Vec<u8>, CompressionMethod);

fn raw_zip(path: &Path, entries: &[Entry<'_>]) {
    let mut z = ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, data, m) in entries {
        z.start_file(*name, SimpleFileOptions::default().compression_method(*m))
            .unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap();
}

fn manifest(files: &[(&str, &[u8])], schema: i64) -> Vec<u8> {
    let files: Vec<Value> = files
        .iter()
        .map(|(p, b)| json!({"path": p, "sha256": sha(b), "bytes": b.len()}))
        .collect();
    serde_json::to_vec_pretty(&json!({
        "format": "qrtz-backup", "version": 1, "createdAt": 1, "appVersion": "t",
        "schemaVersion": schema, "meetings": 1, "files": files,
    }))
    .unwrap()
}

fn snapshot_db(user_version: Option<u32>) -> Vec<u8> {
    let d = tempfile::tempdir().unwrap();
    let db = qrtz_storage::Db::open(d.path().join("a.db")).unwrap();
    db.notes()
        .upsert_meeting(&serde_json::from_value(json!({
            "id": "x1", "title": "t", "platform": "zoom", "startedAt": 1, "status": "done", "participants": [],
            "audioPath": "/elsewhere/x1.wav"
        })).unwrap())
        .unwrap();
    if let Some(v) = user_version {
        db.connection()
            .execute_batch(&format!("PRAGMA user_version = {v}"))
            .unwrap();
    }
    let snap = d.path().join("s.db");
    db.snapshot_to(&snap).unwrap();
    std::fs::read(snap).unwrap()
}

/// Replaces every occurrence of `from` with `to` (same length) in the file: forges names/sizes zip writers won't.
fn patch(path: &Path, from: &[u8], to: &[u8]) {
    assert_eq!(from.len(), to.len());
    let mut b = std::fs::read(path).unwrap();
    let mut i = 0;
    let mut n = 0;
    while i + from.len() <= b.len() {
        if &b[i..i + from.len()] == from {
            b[i..i + from.len()].copy_from_slice(to);
            n += 1;
        }
        i += 1;
    }
    assert!(n > 0);
    std::fs::write(path, b).unwrap();
}

fn generate(dir: &Path) -> Vec<Value> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let (s, d) = (CompressionMethod::Stored, CompressionMethod::Deflated);
    let db = snapshot_db(None);
    let wav = vec![7u8; 4096];
    let ok_manifest = |extra: &[(&str, &[u8])]| {
        let mut f: Vec<(&str, &[u8])> = vec![("db/qrtz.db", &db)];
        f.extend_from_slice(extra);
        manifest(&f, 4)
    };
    let mut cases = Vec::new();
    let mut case = |file: &str, error: &str, stage: &str, why: &str| {
        cases.push(json!({"file": file, "error": error, "stage": stage, "why": why}));
    };

    std::fs::write(dir.join("not-a-zip.zip"), b"this is not a zip file at all").unwrap();
    case(
        "not-a-zip.zip",
        "Backup rejected: not a zip file",
        "read",
        "plain text",
    );

    let electron = std::fs::read(fixtures().join("electron-v1.zip")).unwrap();
    std::fs::write(dir.join("truncated.zip"), &electron[..electron.len() / 2]).unwrap();
    case(
        "truncated.zip",
        "Backup rejected:",
        "read",
        "first half of a valid backup (no central directory)",
    );

    raw_zip(&dir.join("no-manifest.zip"), &[("db/qrtz.db", db.clone(), d)]);
    case(
        "no-manifest.zip",
        "Backup rejected: manifest.json missing",
        "read",
        "no manifest.json",
    );

    raw_zip(
        &dir.join("bad-manifest.zip"),
        &[
            ("db/qrtz.db", db.clone(), d),
            ("manifest.json", b"{not json".to_vec(), d),
        ],
    );
    case(
        "bad-manifest.zip",
        "Backup rejected: manifest is invalid",
        "read",
        "manifest is not JSON",
    );

    let wrong = String::from_utf8(ok_manifest(&[]))
        .unwrap()
        .replace("qrtz-backup", "qrtz-backup2");
    raw_zip(
        &dir.join("wrong-format.zip"),
        &[
            ("db/qrtz.db", db.clone(), d),
            ("manifest.json", wrong.into_bytes(), d),
        ],
    );
    case(
        "wrong-format.zip",
        "Backup rejected: manifest is invalid",
        "read",
        "format is not qrtz-backup",
    );

    let mut huge = b"{\"format\":\"qrtz-backup\",\"pad\":\"".to_vec();
    huge.extend(std::iter::repeat_n(b' ', 17 * 1024 * 1024));
    huge.extend(b"\"}");
    raw_zip(
        &dir.join("huge-manifest.zip"),
        &[("db/qrtz.db", db.clone(), d), ("manifest.json", huge, d)],
    );
    case(
        "huge-manifest.zip",
        "Backup rejected: manifest.json missing",
        "read",
        "manifest over 16 MiB",
    );

    raw_zip(
        &dir.join("zip-slip.zip"),
        &[
            ("db/qrtz.db", db.clone(), d),
            ("../../evil.wav", wav.clone(), s),
            ("manifest.json", ok_manifest(&[("../../evil.wav", &wav)]), d),
        ],
    );
    case(
        "zip-slip.zip",
        "Backup rejected:",
        "read",
        "traversal in an entry the manifest lists",
    );

    raw_zip(
        &dir.join("audio-slip.zip"),
        &[
            ("db/qrtz.db", db.clone(), d),
            ("audio/../x.wav", wav.clone(), s),
            ("manifest.json", ok_manifest(&[("audio/../x.wav", &wav)]), d),
        ],
    );
    case(
        "audio-slip.zip",
        "Backup rejected:",
        "read",
        "audio/../ traversal",
    );

    // Absolute and backslash names: written under a same-length placeholder, then patched.
    raw_zip(
        &dir.join("absolute-path.zip"),
        &[
            ("db/qrtz.db", db.clone(), d),
            ("Xtmp/evil.wav", wav.clone(), s),
            ("manifest.json", ok_manifest(&[("/tmp/evil.wav", &wav)]), d),
        ],
    );
    patch(&dir.join("absolute-path.zip"), b"Xtmp/evil.wav", b"/tmp/evil.wav");
    case(
        "absolute-path.zip",
        "Backup rejected:",
        "read",
        "absolute entry name",
    );

    raw_zip(
        &dir.join("backslash.zip"),
        &[
            ("db/qrtz.db", db.clone(), d),
            ("audioXevil.wav", wav.clone(), s),
            ("manifest.json", ok_manifest(&[("audio\\evil.wav", &wav)]), d),
        ],
    );
    patch(&dir.join("backslash.zip"), b"audioXevil.wav", b"audio\\evil.wav");
    case(
        "backslash.zip",
        "Backup rejected:",
        "read",
        "backslash in an entry name",
    );

    raw_zip(
        &dir.join("unexpected-entry.zip"),
        &[
            ("db/qrtz.db", db.clone(), d),
            ("evil.sh", b"rm -rf ~".to_vec(), d),
            ("manifest.json", ok_manifest(&[]), d),
        ],
    );
    case(
        "unexpected-entry.zip",
        "Backup rejected: unexpected file evil.sh",
        "read",
        "entry the manifest doesn't list",
    );

    let mut m: Value = serde_json::from_slice(&ok_manifest(&[("audio/a.wav", &wav)])).unwrap();
    m["files"][1]["sha256"] = json!("0".repeat(64));
    raw_zip(
        &dir.join("bad-hash.zip"),
        &[
            ("db/qrtz.db", db.clone(), d),
            ("audio/a.wav", wav.clone(), s),
            ("manifest.json", serde_json::to_vec(&m).unwrap(), d),
        ],
    );
    case(
        "bad-hash.zip",
        "Backup rejected: audio/a.wav failed its integrity check",
        "read",
        "sha256 mismatch",
    );

    let mut m: Value = serde_json::from_slice(&ok_manifest(&[("audio/a.wav", &wav)])).unwrap();
    m["files"][1]["bytes"] = json!(wav.len() + 1);
    raw_zip(
        &dir.join("size-mismatch.zip"),
        &[
            ("db/qrtz.db", db.clone(), d),
            ("audio/a.wav", wav.clone(), s),
            ("manifest.json", serde_json::to_vec(&m).unwrap(), d),
        ],
    );
    case(
        "size-mismatch.zip",
        "Backup rejected: audio/a.wav failed its integrity check",
        "read",
        "manifest size differs",
    );

    raw_zip(
        &dir.join("db-not-listed.zip"),
        &[
            ("audio/a.wav", wav.clone(), s),
            ("manifest.json", manifest(&[("audio/a.wav", &wav)], 4), d),
        ],
    );
    case(
        "db-not-listed.zip",
        "Backup rejected: database missing from manifest",
        "read",
        "no db/qrtz.db",
    );

    // Bomb: 64 MiB of zeros deflates to ~64 KiB; its headers and the manifest claim 4096 bytes. Both readers stop
    // at the declared size.
    let zeros = vec![0u8; 64 << 20];
    let small = vec![0u8; 4096];
    let mut m: Value = serde_json::from_slice(&ok_manifest(&[("audio/z.wav", &small)])).unwrap();
    m["files"][1]["sha256"] = json!(sha(&small));
    raw_zip(
        &dir.join("bomb.zip"),
        &[
            ("db/qrtz.db", db.clone(), d),
            ("audio/z.wav", zeros, d),
            ("manifest.json", serde_json::to_vec(&m).unwrap(), d),
        ],
    );
    let size64 = (64u32 << 20).to_le_bytes();
    patch(&dir.join("bomb.zip"), &size64, &4096u32.to_le_bytes());
    case(
        "bomb.zip",
        "Backup rejected:",
        "read",
        "entry inflates far past its declared size",
    );

    let garbage = b"SQLite format 3\0 but not really a database, just garbage bytes".repeat(64);
    raw_zip(
        &dir.join("corrupt-db.zip"),
        &[
            ("db/qrtz.db", garbage.clone(), d),
            ("manifest.json", manifest(&[("db/qrtz.db", &garbage)], 4), d),
        ],
    );
    case(
        "corrupt-db.zip",
        "not a database",
        "import",
        "db entry passes its hash but is not SQLite",
    );

    let newer = snapshot_db(Some(999));
    raw_zip(
        &dir.join("newer-schema.zip"),
        &[
            ("db/qrtz.db", newer.clone(), d),
            ("manifest.json", manifest(&[("db/qrtz.db", &newer)], 999), d),
        ],
    );
    case(
        "newer-schema.zip",
        "backup is from a newer qrtz (schema 999)",
        "import",
        "refused like 520f16a",
    );

    std::fs::write(
        dir.join("cases.json"),
        serde_json::to_string_pretty(&json!(cases)).unwrap() + "\n",
    )
    .unwrap();
    cases
}

#[test]
fn malicious_and_corrupt_archives_are_rejected_leaving_nothing() {
    let dir = fixtures().join("malicious");
    if update() || !dir.join("cases.json").exists() {
        generate(&dir);
    }
    let cases: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("cases.json")).unwrap()).unwrap();
    assert!(cases.len() >= 12);
    for c in cases {
        let file = c["file"].as_str().unwrap();
        let lib = Lib::new();
        let err = lib.import(&dir.join(file)).expect_err(file);
        let want = c["error"].as_str().unwrap();
        assert!(err.to_string().contains(want), "{file}: {err} (want {want})");
        assert!(lib.db.notes().list_meetings(10, 0).unwrap().is_empty(), "{file}");
        assert!(lib.rec_files().is_empty(), "{file}: {:?}", lib.rec_files());
        assert!(lib.tmp_files().is_empty(), "{file}: {:?}", lib.tmp_files());
    }
}

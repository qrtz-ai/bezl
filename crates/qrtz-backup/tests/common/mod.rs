#![allow(dead_code)]
use std::path::{Path, PathBuf};

use qrtz_backup::{import_backup, no_progress, ImportBackup, ImportLimits, ImportReport};
use qrtz_storage::model::Meeting;
use qrtz_storage::{Db, PendingDeletes, StorageError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/fixtures/backup")
}

pub fn update() -> bool {
    std::env::var("QRTZ_FIXTURES_UPDATE").as_deref() == Ok("1")
}

pub fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// A fresh app data dir: `qrtz.db`, `recordings/`, `tmp/`.
pub struct Lib {
    pub dir: tempfile::TempDir,
    pub db: Db,
}

impl Lib {
    pub fn new() -> Lib {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("tmp")).unwrap();
        let db = Db::open(dir.path().join("qrtz.db")).unwrap();
        Lib { dir, db }
    }
    pub fn rec(&self) -> PathBuf {
        self.dir.path().join("recordings")
    }
    pub fn tmp(&self) -> PathBuf {
        self.dir.path().join("tmp")
    }
    pub fn import_with(
        &self,
        zip: &Path,
        pending: Option<&PendingDeletes>,
        limits: ImportLimits,
    ) -> Result<ImportReport, StorageError> {
        import_backup(
            &ImportBackup {
                db: &self.db,
                source: zip,
                recordings_dir: &self.rec(),
                temp_dir: &self.tmp(),
                pending,
                limits,
            },
            &mut no_progress,
        )
    }
    pub fn import(&self, zip: &Path) -> Result<ImportReport, StorageError> {
        self.import_with(zip, None, ImportLimits::default())
    }
    /// Files in recordings/ (names), sorted.
    pub fn rec_files(&self) -> Vec<String> {
        ls(&self.rec())
    }
    pub fn tmp_files(&self) -> Vec<String> {
        ls(&self.tmp())
    }
}

pub fn ls(p: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(p)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Same shape as `dump` in apps/desktop/src/main/backup.parity.test.ts.
pub fn dump(db: &Db) -> Value {
    let n = db.notes();
    let mut ms: Vec<Meeting> = n.list_meetings(1_000_000, 0).unwrap();
    ms.sort_by(|a, b| a.id.cmp(&b.id));
    Value::Array(
        ms.into_iter()
            .map(|m| {
                let audio = m.audio_path.as_ref().map(|p| {
                    let p = Path::new(p);
                    json!({
                        "name": p.file_name().unwrap().to_string_lossy(),
                        "sha256": std::fs::read(p).ok().map(|b| sha(&b)),
                    })
                });
                let mut rest = serde_json::to_value(&m).unwrap();
                rest.as_object_mut().unwrap().remove("audioPath");
                json!({
                    "meeting": rest,
                    "audio": audio,
                    "segments": n.get_segments(&m.id).unwrap(),
                    "summary": n.get_summary(&m.id).unwrap(),
                    "notes": n.get_notes(&m.id).unwrap(),
                })
            })
            .collect(),
    )
}

pub fn expected() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixtures().join("expected.json")).unwrap()).unwrap()
}

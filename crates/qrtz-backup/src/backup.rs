//! Backup zip export and import: the port of `writeBackup` / `readBackup` (`apps/desktop/src/main/backup.ts`) and
//! the `data:export-backup` / `data:import-backup` handlers, in the same file format (see [`crate::format`]).
//! Everything streams: memory stays flat whatever the library size.
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};

use qrtz_model::Meeting;
use qrtz_storage::{Db, ImportHooks, PendingDeletes, StorageError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::format::{
    is_safe_audio, is_safe_entry, Manifest, ManifestFile, BACKUP_FORMAT, BACKUP_VERSION, DB_ENTRY,
    MANIFEST_ENTRY,
};

type Result<T> = std::result::Result<T, StorageError>;

/// Progress phases, as `data:backup-progress` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Export: streaming entries into the zip (hashing happens in the same pass).
    Write,
    /// Import: streaming entries out of the zip and checking their hashes.
    Verify,
    /// Import: merging meetings into the database.
    Import,
}

/// `(phase, done, total)`; byte counts for `Write`/`Verify`, meetings for `Import`.
pub type Progress<'a> = &'a mut dyn FnMut(Phase, u64, u64);

/// A progress sink that ignores everything.
pub fn no_progress(_: Phase, _: u64, _: u64) {}

fn bad(msg: impl std::fmt::Display) -> StorageError {
    StorageError::Validation(format!("Backup rejected: {msg}"))
}

// ------------------------------------------------------------------ export

/// `writeBackup` inputs.
pub struct WriteBackup<'a> {
    pub target: &'a Path,
    pub db_snapshot: &'a Path,
    pub audio_files: &'a [PathBuf],
    pub app_version: &'a str,
    pub schema_version: i64,
    pub meetings: i64,
    /// `createdAt` and the entries' zip timestamps (epoch ms).
    pub now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteReport {
    pub bytes: u64,
    pub audio_files: u32,
}

/// Zips the snapshot and the audio files with a sha256 manifest. Each file is read once: hashed while it is
/// written, so a recording that grows during the export still matches its manifest line. The archive is written
/// to `<target>.partial` (0600, a stale one removed first) and renamed into place only when complete.
pub fn write_backup(o: &WriteBackup<'_>, progress: Progress<'_>) -> Result<WriteReport> {
    let mut entries: Vec<(String, &Path)> = vec![(DB_ENTRY.to_string(), o.db_snapshot)];
    for f in o.audio_files {
        let Some(name) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let entry = format!("audio/{name}");
        if is_safe_audio(&entry) && f.is_file() {
            entries.push((entry, f));
        }
    }
    let total: u64 = entries
        .iter()
        .map(|(_, f)| fs::metadata(f).map(|m| m.len()).unwrap_or(0))
        .sum();
    if let Some(dir) = o.target.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let tmp = PathBuf::from(format!("{}.partial", o.target.display()));
    let _ = fs::remove_file(&tmp);
    let res = (|| -> Result<u32> {
        let file = create_owner_only(&tmp)?;
        let mut zip = ZipWriter::new(BufWriter::with_capacity(1 << 16, file));
        let stamp = zip_time(o.now_ms);
        let mut files = Vec::new();
        let mut done = 0u64;
        let mut audio = 0u32;
        for (entry, path) in &entries {
            let src = match File::open(path) {
                Ok(f) => f,
                // A recording removed since the listing (deleted meeting) is skipped, like TS `existsSync`.
                Err(e) if e.kind() == io::ErrorKind::NotFound && entry != DB_ENTRY => continue,
                Err(e) => return Err(e.into()),
            };
            let len = src.metadata()?.len();
            let opts = SimpleFileOptions::default()
                .compression_method(if entry == DB_ENTRY {
                    CompressionMethod::Deflated
                } else {
                    CompressionMethod::Stored
                })
                .last_modified_time(stamp)
                .unix_permissions(0o600)
                // Zip64 headers when the file is near 4 GiB (+ slack for growth while we read it).
                .large_file(len >= (u32::MAX as u64) / 2);
            zip.start_file(entry.as_str(), opts).map_err(zip_err)?;
            let mut h = Sha256::new();
            let mut r = BufReader::with_capacity(1 << 16, src);
            let mut buf = vec![0u8; 1 << 16];
            let mut bytes = 0u64;
            loop {
                let n = r.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
                zip.write_all(&buf[..n])?;
                bytes += n as u64;
                done += n as u64;
                progress(Phase::Write, done.min(total), total);
            }
            files.push(ManifestFile {
                path: entry.clone(),
                sha256: hex(&h.finalize()),
                bytes,
            });
            if entry != DB_ENTRY {
                audio += 1;
            }
        }
        let manifest = Manifest {
            format: BACKUP_FORMAT.into(),
            version: BACKUP_VERSION,
            created_at: o.now_ms.into(),
            app_version: o.app_version.into(),
            schema_version: o.schema_version,
            meetings: o.meetings,
            files,
        };
        zip.start_file(
            MANIFEST_ENTRY,
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .last_modified_time(stamp)
                .unix_permissions(0o600),
        )
        .map_err(zip_err)?;
        zip.write_all(manifest.to_json().as_bytes())?;
        let mut w = zip.finish().map_err(zip_err)?;
        w.flush()?;
        w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        Ok(audio)
    })();
    match res {
        Ok(audio_files) => {
            if let Err(e) = fs::rename(&tmp, o.target) {
                let _ = fs::remove_file(&tmp);
                return Err(e.into());
            }
            Ok(WriteReport {
                bytes: fs::metadata(o.target)?.len(),
                audio_files,
            })
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// `data:export-backup` inputs (after the shell picked `target`).
pub struct ExportBackup<'a> {
    pub db: &'a Db,
    pub target: &'a Path,
    /// `<data>/recordings`: every `*.wav` in it goes into the archive.
    pub recordings_dir: &'a Path,
    /// Where the private (0700) snapshot directory is made; removed before returning.
    pub temp_dir: &'a Path,
    pub app_version: &'a str,
    pub now_ms: i64,
}

/// `data:export-backup` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub saved_to: String,
    pub meetings: u32,
    pub audio_files: u32,
    pub bytes: u64,
}

/// Snapshot (`VACUUM INTO`, 0600 inside a 0700 temp dir) + recordings → zip at `target`.
pub fn export_backup(o: &ExportBackup<'_>, progress: Progress<'_>) -> Result<ExportReport> {
    let snap_dir = tempfile::Builder::new()
        .prefix("qrtz-snapshot-")
        .tempdir_in(o.temp_dir)?;
    let snap = snap_dir.path().join("qrtz.db");
    o.db.snapshot_to(&snap)?;
    let meetings: u32 =
        o.db.connection()
            .query_row("SELECT count(*) FROM meetings", [], |r| r.get(0))?;
    let mut audio: Vec<PathBuf> = match fs::read_dir(o.recordings_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "wav") && p.is_file())
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    audio.sort();
    let r = write_backup(
        &WriteBackup {
            target: o.target,
            db_snapshot: &snap,
            audio_files: &audio,
            app_version: o.app_version,
            schema_version: i64::from(o.db.schema_version()?),
            meetings: i64::from(meetings),
            now_ms: o.now_ms,
        },
        progress,
    )?;
    Ok(ExportReport {
        saved_to: o.target.to_string_lossy().into_owned(),
        meetings,
        audio_files: r.audio_files,
        bytes: r.bytes,
    })
}

// ------------------------------------------------------------------ import

/// Zip-bomb and size caps for import.
#[derive(Debug, Clone)]
pub struct ImportLimits {
    /// `manifest.json` (TS `MAX_MANIFEST`, 16 MiB).
    pub max_manifest_bytes: u64,
    /// Any single entry, uncompressed.
    pub max_entry_bytes: u64,
    /// All entries together, uncompressed.
    pub max_total_bytes: u64,
    /// Uncompressed/compressed ratio refused for entries over 1 MiB (VACUUM snapshots deflate ~3-10x).
    pub max_ratio: u64,
    /// Test hook: fail staging writes with ENOSPC after this many bytes.
    #[doc(hidden)]
    pub fail_write_after: Option<u64>,
}

impl Default for ImportLimits {
    fn default() -> Self {
        ImportLimits {
            max_manifest_bytes: 16 * 1024 * 1024,
            max_entry_bytes: 64 << 30,
            max_total_bytes: 512 << 30,
            max_ratio: 1000,
            fail_write_after: None,
        }
    }
}

/// `data:import-backup` inputs (after the shell picked `source`).
pub struct ImportBackup<'a> {
    pub db: &'a Db,
    pub source: &'a Path,
    /// `<data>/recordings`: restored audio lands here only (dir 0700, files 0600).
    pub recordings_dir: &'a Path,
    /// Where the private (0700) work directory for the backup DB is made; removed before returning.
    pub temp_dir: &'a Path,
    /// Tombstones (`RuntimeDeps::pending_delete`).
    pub pending: Option<&'a PendingDeletes>,
    pub limits: ImportLimits,
}

/// `data:import-backup` response (minus `canceled`, which only the shell's dialog produces).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported: u32,
    pub skipped: u32,
    pub audio_files: u32,
}

/// Verifies the whole archive before touching the library (manifest, entry names, sizes, sha256, SQLite
/// integrity, schema version), then merges meetings whose id isn't present, one atomic block per meeting with
/// its recording. Nothing of a rejected backup stays on disk.
pub fn import_backup(o: &ImportBackup<'_>, progress: Progress<'_>) -> Result<ImportReport> {
    let file = File::open(o.source).map_err(|_| bad("not a zip file"))?;
    let mut zip = ZipArchive::new(BufReader::new(file)).map_err(|_| bad("not a zip file"))?;
    let work = tempfile::Builder::new()
        .prefix("qrtz-import-")
        .tempdir_in(o.temp_dir)?;
    ensure_private_dir(o.recordings_dir)?;
    let mut staged = Staged::default();
    let r = (|| {
        let manifest = verify_into(&mut zip, o, work.path(), &mut staged, progress)?;
        let db_path = work.path().join("import.db");
        let mut hooks = Hooks {
            staged: &mut staged,
            dir: o.recordings_dir,
            pending: o.pending,
            created: Vec::new(),
            committed: HashMap::new(),
        };
        let total = manifest.meetings.max(0) as u64;
        progress(Phase::Import, 0, total);
        let rep = o.db.import_from_with(&db_path, &mut hooks)?;
        let audio_files = hooks.committed.len() as u32;
        progress(Phase::Import, total, total);
        Ok(ImportReport {
            imported: rep.imported,
            skipped: rep.skipped,
            audio_files,
        })
    })();
    staged.discard();
    r
}

#[derive(Default)]
struct Staged {
    /// backup file name → (verified temp file inside the recordings dir, sha256)
    audio: HashMap<String, (PathBuf, String)>,
    /// every temp file written so far (removed unless renamed into place)
    written: Vec<PathBuf>,
}

impl Staged {
    fn discard(&mut self) {
        for f in self.written.drain(..) {
            let _ = fs::remove_file(f);
        }
    }
}

fn verify_into<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    o: &ImportBackup<'_>,
    work: &Path,
    staged: &mut Staged,
    progress: Progress<'_>,
) -> Result<Manifest> {
    let lim = &o.limits;
    // name → (uncompressed, compressed)
    let mut byname: HashMap<String, (u64, u64)> = HashMap::new();
    for i in 0..zip.len() {
        let e = zip
            .by_index_raw(i)
            .map_err(|e| bad(format!("unreadable zip ({e})")))?;
        let name = String::from_utf8(e.name_raw().to_vec())
            .map_err(|_| bad("unreadable zip (entry name is not UTF-8)"))?;
        if !plausible_name(&name) {
            return Err(bad(format!("unsafe path {name}")));
        }
        if e.encrypted() {
            return Err(bad(format!("unexpected file {name}")));
        }
        if byname
            .insert(name.clone(), (e.size(), e.compressed_size()))
            .is_some()
        {
            return Err(bad(format!("unreadable zip (duplicate entry {name})")));
        }
    }
    let manifest: Manifest = {
        match byname.get(MANIFEST_ENTRY) {
            Some((n, _)) if *n <= lim.max_manifest_bytes => {}
            _ => return Err(bad("manifest.json missing")),
        }
        let mut raw = Vec::new();
        zip.by_name(MANIFEST_ENTRY)
            .map_err(|_| bad("manifest.json missing"))?
            .take(lim.max_manifest_bytes + 1)
            .read_to_end(&mut raw)
            .map_err(|_| bad("manifest is invalid"))?;
        let m: Manifest = serde_json::from_slice(&raw).map_err(|_| bad("manifest is invalid"))?;
        m.validate().map_err(|_| bad("manifest is invalid"))?;
        m
    };
    let mut listed = HashSet::new();
    for f in &manifest.files {
        if !listed.insert(f.path.as_str()) {
            return Err(bad("manifest is invalid"));
        }
    }
    if !listed.contains(DB_ENTRY) {
        return Err(bad("database missing from manifest"));
    }
    for name in byname.keys() {
        if name.ends_with('/') || name == MANIFEST_ENTRY {
            continue;
        }
        if !listed.contains(name.as_str()) {
            return Err(bad(format!("unexpected file {name}")));
        }
    }
    let total = manifest
        .files
        .iter()
        .try_fold(0u64, |n, f| n.checked_add(f.bytes))
        .filter(|t| *t <= lim.max_total_bytes)
        .ok_or_else(|| bad("backup is larger than the import limit"))?;
    let mut done = 0u64;
    let mut written_bytes = 0u64;
    for f in &manifest.files {
        if !is_safe_entry(&f.path) {
            return Err(bad(format!("unsafe path {}", f.path)));
        }
        let Some(&(size, csize)) = byname.get(&f.path) else {
            return Err(bad(format!("{} missing", f.path)));
        };
        if size != f.bytes {
            return Err(bad(format!("{} failed its integrity check", f.path)));
        }
        if size > lim.max_entry_bytes || (size > 1 << 20 && size / csize.max(1) > lim.max_ratio) {
            return Err(bad(format!("{} is larger than the import limit", f.path)));
        }
        let base = f.path.rsplit('/').next().unwrap_or_default().to_string();
        let tmp = if f.path == DB_ENTRY {
            work.join("import.db")
        } else {
            o.recordings_dir
                .join(format!(".{base}.{}.partial", std::process::id()))
        };
        let _ = fs::remove_file(&tmp);
        let out = create_owner_only(&tmp)?;
        staged.written.push(tmp.clone());
        let mut w = BufWriter::with_capacity(1 << 16, out);
        let mut entry = zip
            .by_name(&f.path)
            .map_err(|_| bad(format!("{} missing", f.path)))?;
        let mut h = Sha256::new();
        let mut bytes = 0u64;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = entry
                .read(&mut buf)
                .map_err(|_| bad(format!("{} is corrupt and failed its integrity check", f.path)))?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            if bytes > f.bytes {
                return Err(bad(format!(
                    "{} is corrupt and failed its integrity check",
                    f.path
                )));
            }
            h.update(&buf[..n]);
            written_bytes += n as u64;
            if lim.fail_write_after.is_some_and(|cap| written_bytes > cap) {
                return Err(io::Error::from_raw_os_error(28).into()); // ENOSPC
            }
            w.write_all(&buf[..n])?;
            done += n as u64;
            progress(Phase::Verify, done, total);
        }
        w.flush()?;
        w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        let sha = hex(&h.finalize());
        if bytes != f.bytes || sha != f.sha256 {
            return Err(bad(format!("{} failed its integrity check", f.path)));
        }
        if f.path != DB_ENTRY {
            staged.audio.insert(base, (tmp, sha));
        }
    }
    Ok(manifest)
}

/// Places each imported meeting's recording just before its transaction; undoes it if the transaction fails.
struct Hooks<'a> {
    staged: &'a mut Staged,
    dir: &'a Path,
    pending: Option<&'a PendingDeletes>,
    /// files this meeting created (removed on rollback)
    created: Vec<(String, PathBuf)>,
    /// backup file name → restored path
    committed: HashMap<String, PathBuf>,
}

impl ImportHooks for Hooks<'_> {
    fn audio(&mut self, m: &Meeting, base: &str) -> qrtz_storage::Result<Option<String>> {
        self.created.clear();
        self.clear_stale_tombstone(&m.id)?;
        if let Some(p) = self.committed.get(base) {
            return Ok(Some(p.to_string_lossy().into_owned()));
        }
        let Some((tmp, sha)) = self.staged.audio.remove(base) else {
            return Ok(None); // not in the archive: the path is dropped, like TS
        };
        let mut target = self.dir.join(base);
        if target.exists() && hash_file(&target)? != sha {
            let stem = base.strip_suffix(".wav").unwrap_or(base);
            target = self.dir.join(format!("{stem}-restored-{}.wav", now_ms()));
        }
        if target.exists() {
            let _ = fs::remove_file(&tmp); // identical recording already here
        } else {
            fs::rename(&tmp, &target)?;
            self.created.push((base.to_string(), target.clone()));
        }
        self.committed.insert(base.to_string(), target.clone());
        Ok(Some(target.to_string_lossy().into_owned()))
    }

    fn skip(&mut self, _id: &str) -> bool {
        false
    }

    fn rolled_back(&mut self, _m: &Meeting) {
        for (base, p) in self.created.drain(..) {
            let _ = fs::remove_file(&p);
            self.committed.remove(&base);
        }
    }
}

impl Hooks<'_> {
    /// A meeting that is still in the library is skipped (its tombstone, if any, then deletes it as scheduled).
    /// One that is gone but still has a tombstone is a delete that finished without clearing it: drop the stale
    /// tombstone first, or the next launch's recovery would delete the meeting this import restores.
    fn clear_stale_tombstone(&self, id: &str) -> qrtz_storage::Result<()> {
        if let Some(pd) = self.pending {
            if pd.has(id) {
                pd.remove(id)?;
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ helpers

/// No absolute names, drive letters, backslashes or `.`/`..`/empty components in any entry, listed or not
/// (yauzl's `strictFileNames` + its relative-path check on the TS side).
fn plausible_name(n: &str) -> bool {
    let body = n.strip_suffix('/').unwrap_or(n);
    !n.starts_with('/')
        && !n.contains('\\')
        && !n.contains(':')
        && !body.is_empty()
        && body.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

fn zip_err(e: zip::result::ZipError) -> StorageError {
    match e {
        zip::result::ZipError::Io(e) => e.into(),
        e => StorageError::Io(io::Error::other(e.to_string())),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Streams a file through sha256.
pub fn hash_file(p: &Path) -> io::Result<String> {
    let mut r = BufReader::with_capacity(1 << 16, File::open(p)?);
    let mut h = Sha256::new();
    io::copy(&mut r, &mut h)?;
    Ok(hex(&h.finalize()))
}

fn create_owner_only(p: &Path) -> io::Result<File> {
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(p)
}

fn ensure_private_dir(p: &Path) -> io::Result<()> {
    fs::create_dir_all(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn zip_time(ms: i64) -> zip::DateTime {
    let secs = ms.div_euclid(1000);
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    zip::DateTime::from_date_and_time(
        y.clamp(1980, 2107) as u16,
        mo as u8,
        d as u8,
        (sod / 3600) as u8,
        (sod / 60 % 60) as u8,
        (sod % 60) as u8,
    )
    .unwrap_or_default()
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

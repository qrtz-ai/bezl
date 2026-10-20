#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Backlog P2 (ADR 0218): a download cut short by a crash, a kill or a quit resumes from its `.part` and `.done`
//! ranges in the next process, and the result is checked against the pinned sha256. Echo's first Parakeet download
//! (each model file has one pinned source) goes through this fetcher.
use qrtz_modules::fetch::{fetch_blobs, Source, Transport, RANGE_BYTES};
use qrtz_modules::index::BlobRef;
use qrtz_modules::store::Store;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MIRROR: &str = "https://huggingface.co/r/resolve/abc/model.bin";

/// The blob: `ranges` ranges, the last one short.
fn data(ranges: u64) -> Vec<u8> {
    let size = (ranges - 1) * RANGE_BYTES + 12345;
    (0..size)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect()
}

fn blob(data: &[u8]) -> BlobRef {
    BlobRef {
        sha256: qrtz_modules::sha256_hex(data),
        size: data.len() as u64,
        mirrors: vec![MIRROR.into()],
    }
}

#[derive(Clone, Copy)]
enum Serve {
    /// Every range.
    All,
    /// Only these ranges; the rest fail like a dropped connection.
    Only(&'static [u64]),
    /// Ranges below this; the rest never answer (the process is killed while they hang).
    HangFrom(u64),
    /// The right length, the wrong bytes.
    Wrong,
}

/// The pinned mirror (the only source), recording each range it was asked for.
struct Mirror {
    data: Vec<u8>,
    serve: Serve,
    asked: Mutex<Vec<u64>>,
}

impl Mirror {
    fn new(data: &[u8], serve: Serve) -> Self {
        Mirror {
            data: data.to_vec(),
            serve,
            asked: Mutex::new(vec![]),
        }
    }
    fn asked(&self) -> Vec<u64> {
        let mut v = self.asked.lock().unwrap().clone();
        v.sort();
        v
    }
}

#[async_trait::async_trait]
impl Transport for Mirror {
    fn hosts(&self) -> usize {
        0
    }
    async fn get(&self, _host: usize, _path: &str, _max: usize) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    async fn get_range(&self, src: &Source, _sha256: &str, start: u64, len: u64) -> Result<Vec<u8>, String> {
        assert_eq!(src, &Source::Url(MIRROR.into()));
        let i = start / RANGE_BYTES;
        self.asked.lock().unwrap().push(i);
        let bytes = self.data[start as usize..(start + len) as usize].to_vec();
        match self.serve {
            Serve::All => Ok(bytes),
            Serve::Only(r) if r.contains(&i) => Ok(bytes),
            Serve::Only(_) => Err("connection reset".into()),
            Serve::HangFrom(k) if i < k => Ok(bytes),
            Serve::HangFrom(_) => std::future::pending().await,
            Serve::Wrong => Ok(bytes.iter().map(|b| b ^ 0x5a).collect()),
        }
    }
}

fn part(store: &Store, b: &BlobRef) -> PathBuf {
    store.tmp().join(format!("{}.part", b.sha256))
}
fn marks(store: &Store, b: &BlobRef) -> PathBuf {
    store.tmp().join(format!("{}.done", b.sha256))
}

/// A part as an earlier process left it: full size, `written` ranges holding `bytes_of(i)`, the rest zeros.
fn leave_part(store: &Store, b: &BlobRef, written: &[(u64, Vec<u8>)], marks_text: &str) {
    std::fs::create_dir_all(store.tmp()).unwrap();
    let f = std::fs::File::create(part(store, b)).unwrap();
    f.set_len(b.size).unwrap();
    use std::os::unix::fs::FileExt;
    for (i, bytes) in written {
        f.write_all_at(bytes, i * RANGE_BYTES).unwrap();
    }
    std::fs::write(marks(store, b), marks_text).unwrap();
}

fn range(data: &[u8], i: u64) -> Vec<u8> {
    let s = (i * RANGE_BYTES) as usize;
    data[s..(s + RANGE_BYTES as usize).min(data.len())].to_vec()
}

/// Every line of the `.done` file, as written (whole lines only).
fn mark_lines(store: &Store, b: &BlobRef) -> Vec<String> {
    std::fs::read_to_string(marks(store, b))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn a_torn_range_mark_never_merges_with_the_next_one() {
    // A kill between a mark's digits and its newline ("1" with no "\n"; before, the digits and the newline were two
    // writes). The resumed fetch lands range 0 and then the network drops: the old code appended "0\n" to the torn
    // "1", so the marks read "10", range 10 counted as on disk while it was zeros, and the next resume finished a
    // part that failed its hash with no other source: the whole 80 MB download failed its check.
    let data = data(11);
    let b = blob(&data);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store"));
    leave_part(&store, &b, &[(1, range(&data, 1))], "1");

    let cut = Mirror::new(&data, Serve::Only(&[0]));
    assert!(fetch_blobs(&cut, &store, std::slice::from_ref(&b), &|_, _| {})
        .await
        .is_err());
    let lines = mark_lines(&store, &b);
    let on_disk = std::fs::read(part(&store, &b)).unwrap();
    for l in &lines {
        let i: u64 = l
            .parse()
            .unwrap_or_else(|_| panic!("a mark that isn't a range: {l:?} ({lines:?})"));
        assert!(i < 11, "a mark past the last range: {i} ({lines:?})");
        assert_eq!(
            range(&on_disk, i),
            range(&data, i),
            "range {i} is marked done but its bytes aren't on disk ({lines:?})"
        );
    }

    let next = Mirror::new(&data, Serve::All);
    fetch_blobs(&next, &store, std::slice::from_ref(&b), &|_, _| {})
        .await
        .expect("the resume finishes and matches the pinned sha256");
    assert!(store.verify(&b.sha256, b.size).unwrap());
    // Range 0 resumed; the torn mark's range 1 is fetched again; nothing is fetched twice.
    assert_eq!(next.asked(), (1..11).collect::<Vec<_>>());
    assert!(!part(&store, &b).exists() && !marks(&store, &b).exists());
}

#[tokio::test]
async fn a_resumed_part_that_fails_its_hash_starts_over_once_from_its_only_source() {
    // A mark outlived its bytes (range 1 marked done, zeros on disk: a crash's lost write). A model file has one
    // pinned source, so before, the finished part failed its hash and the download failed as a checksum error.
    let data = data(3);
    let b = blob(&data);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store"));
    leave_part(&store, &b, &[(0, range(&data, 0))], "0\n1\n");
    let m = Mirror::new(&data, Serve::All);
    fetch_blobs(&m, &store, std::slice::from_ref(&b), &|_, _| {})
        .await
        .expect("one fresh try from the same source");
    assert!(store.verify(&b.sha256, b.size).unwrap());
    // Range 2 for the resume, then all three once.
    assert_eq!(m.asked(), vec![0, 1, 2, 2]);
}

#[tokio::test]
async fn a_source_serving_wrong_bytes_still_fails_after_one_fresh_try() {
    // Guard: the fresh try is once, and only for a part kept from an earlier fetch.
    let data = data(3);
    let b = blob(&data);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store"));
    leave_part(&store, &b, &[(0, range(&data, 0))], "0\n");
    let m = Mirror::new(&data, Serve::Wrong);
    let e = fetch_blobs(&m, &store, std::slice::from_ref(&b), &|_, _| {})
        .await
        .unwrap_err();
    assert!(e.contains(qrtz_modules::fetch::MISMATCH), "{e}");
    assert_eq!(m.asked(), vec![0, 1, 1, 2, 2]);
    assert!(!store.has(&b.sha256, b.size));

    // A fresh download whose bytes are wrong is not tried twice.
    let fresh = Store::new(dir.path().join("fresh"));
    let m = Mirror::new(&data, Serve::Wrong);
    assert!(fetch_blobs(&m, &fresh, std::slice::from_ref(&b), &|_, _| {})
        .await
        .is_err());
    assert_eq!(m.asked(), vec![0, 1, 2]);
}

#[tokio::test]
async fn a_part_a_crash_left_read_only_is_written_again() {
    // A crash between `insert_file`'s chmod 0444 and its rename, with marks lost: before, every range write failed
    // "permission denied" on each retry, forever.
    use std::os::unix::fs::PermissionsExt;
    let data = data(3);
    let b = blob(&data);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store"));
    leave_part(&store, &b, &[(0, range(&data, 0))], "0\n");
    std::fs::set_permissions(part(&store, &b), std::fs::Permissions::from_mode(0o444)).unwrap();
    let m = Mirror::new(&data, Serve::All);
    fetch_blobs(&m, &store, std::slice::from_ref(&b), &|_, _| {})
        .await
        .expect("the kept part is made writable");
    assert!(store.verify(&b.sha256, b.size).unwrap());
    assert_eq!(m.asked(), vec![1, 2]);
}

const CHILD: &str = "QRTZ_FETCH_KILL_CHILD_DIR";

/// The child: fetch into `dir` from a mirror that serves ranges 0 and 1 and never answers the rest.
fn child(dir: &Path) -> ! {
    let data = data(6);
    let store = Store::new(dir.join("store"));
    let m = Mirror::new(&data, Serve::HangFrom(2));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _ = rt.block_on(fetch_blobs(&m, &store, &[blob(&data)], &|_, _| {}));
    unreachable!("ranges 2.. never answer");
}

#[test]
fn a_download_killed_mid_fetch_resumes_from_its_ranges_in_the_next_process() {
    if let Ok(d) = std::env::var(CHILD) {
        child(Path::new(&d));
    }
    let dir = tempfile::tempdir().unwrap();
    let mut kid = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_download_killed_mid_fetch_resumes_from_its_ranges_in_the_next_process",
            "--test-threads=1",
        ])
        .env(CHILD, dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let data = data(6);
    let b = blob(&data);
    let store = Store::new(dir.path().join("store"));
    let deadline = Instant::now() + Duration::from_secs(60);
    while mark_lines(&store, &b).len() < 2 {
        assert!(Instant::now() < deadline, "the child never landed ranges 0 and 1");
        assert!(kid.try_wait().unwrap().is_none(), "the child exited on its own");
        std::thread::sleep(Duration::from_millis(20));
    }
    // SIGKILL: no destructors, no cleanup, exactly what a crash or a force quit leaves.
    kid.kill().unwrap();
    let st = kid.wait().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(st.signal(), Some(9), "{st:?}");
    let kept: BTreeSet<String> = mark_lines(&store, &b).into_iter().collect();
    assert_eq!(kept, BTreeSet::from(["0".into(), "1".into()]));

    // The next launch: only the missing ranges, and the result matches the pinned sha256.
    let m = Mirror::new(&data, Serve::All);
    let seen = Mutex::new(vec![]);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(fetch_blobs(&m, &store, std::slice::from_ref(&b), &|d, t| {
        seen.lock().unwrap().push((d, t))
    }))
    .unwrap();
    assert_eq!(m.asked(), vec![2, 3, 4, 5]);
    assert!(store.verify(&b.sha256, b.size).unwrap());
    let seen = seen.into_inner().unwrap();
    assert_eq!(
        seen.first().unwrap().0,
        2 * RANGE_BYTES,
        "the kept ranges count first"
    );
    assert_eq!(seen.last().unwrap(), &(b.size, b.size));
    assert!(!part(&store, &b).exists() && !marks(&store, &b).exists());
}

//! Cycle 22: an index step never waits out its deadline on a model that is still loading or downloading (N104), the
//! model files are hashed streamed and once per run (N105), the step deadlines sit inside the runtime's RPC
//! timeouts (N3, a TS test lost with Electron), and a full disk during the search-model download leaves nothing
//! behind. Servers are local scripted sockets; nothing touches the network.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::{CoreSteps, InlineDb, StepExecutor, INDEX_TIMEOUT_MS};
use qrtz_model::*;
use qrtz_search::embedder::LoadFuture;
use qrtz_search::*;
use qrtz_storage::Db;
use serde_json::json;

// ------------------------------------------------------------------ the largest allocation, per thread (N105)

struct Track;
thread_local! {
    static BIGGEST: Cell<usize> = const { Cell::new(0) };
}
fn note(n: usize) {
    let _ = BIGGEST.try_with(|b| {
        if n > b.get() {
            b.set(n)
        }
    });
}
unsafe impl GlobalAlloc for Track {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        note(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        note(l.size());
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        note(n);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Track = Track;

fn biggest_during<T>(f: impl FnOnce() -> T) -> (T, usize) {
    BIGGEST.with(|b| b.set(0));
    let r = f();
    (r, BIGGEST.with(|b| b.get()))
}

// ------------------------------------------------------------------ fixtures

struct Fake;
#[async_trait]
impl Embedder for Fake {
    fn id(&self) -> &str {
        "fake-2d"
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError> {
        Ok(texts.iter().map(|_| vec![0.6, 0.8]).collect())
    }
}

fn db_with(id: &str, n: usize) -> InlineDb {
    let db = Db::open_in_memory().unwrap();
    let m: Meeting = serde_json::from_value(json!({
        "id": id, "title": "Standup", "platform": "zoom", "startedAt": 1, "status": "done", "participants": []
    }))
    .unwrap();
    db.notes().upsert_meeting(&m).unwrap();
    let segs: Vec<Segment> = (0..n)
        .map(|i| {
            serde_json::from_value(
                json!({"id": format!("{id}-{i}"), "startMs": i * 1000, "endMs": i * 1000 + 900,
                "text": format!("budget line {i}"), "speakerName": "Ana", "source": "live"}),
            )
            .unwrap()
        })
        .collect();
    db.notes()
        .replace_segments(id, SegmentSource::Live, &segs)
        .unwrap();
    InlineDb::new(db)
}

fn vectors(db: &InlineDb, model: &str) -> usize {
    db.0.lock()
        .unwrap()
        .notes()
        .vector_sources(model, None)
        .unwrap()
        .len()
}

fn steps(db: &InlineDb, e: Arc<LazyEmbedder>) -> SearchSteps<InlineDb> {
    SearchSteps::new(db.clone(), Arc::new(CoreSteps { db: db.clone() }), e)
}

/// A loader that finishes when `gate` opens (a first-run model download).
fn gated_loader(gate: Arc<tokio::sync::Notify>) -> Arc<LazyEmbedder> {
    LazyEmbedder::new(Box::new(move || -> LoadFuture {
        let g = gate.clone();
        Box::pin(async move {
            g.notified().await;
            Ok(Arc::new(Fake) as Arc<dyn Embedder>)
        })
    }))
}

// ------------------------------------------------------------------ N104

/// A model that loads inside the wait (a local load: a second or two) is still used by that step.
#[tokio::test(start_paused = true)]
async fn n104_a_model_that_loads_inside_the_wait_is_used() {
    let db = db_with("a", 3);
    let lazy = LazyEmbedder::new(Box::new(|| -> LoadFuture {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok(Arc::new(Fake) as Arc<dyn Embedder>)
        })
    }));
    let out = steps(&db, lazy).index("a".into()).await.unwrap();
    assert_eq!((out.indexed, out.skipped), (3, false));
}

/// The first-run download outlasts the wait: three attempts used to hold summary + index (one queue across
/// meetings) for up to 3 × 280 s and then fail the index step. Now each attempt skips after the wait, well inside
/// the deadline, and nothing fails.
#[tokio::test(start_paused = true)]
async fn n104_a_downloading_model_never_holds_the_step_queue_past_the_wait() {
    let db = db_with("a", 3);
    let lazy = gated_loader(Arc::new(tokio::sync::Notify::new()));
    let heavy = Arc::new(tokio::sync::Mutex::new(()));
    // Meeting A's index step holds the queue, as `NoteRuntime::exec_step` does; meeting B's summary waits on it.
    let t0 = tokio::time::Instant::now();
    let (h, s) = (heavy.clone(), steps(&db, lazy.clone()));
    let a = tokio::spawn(async move {
        let _turn = h.lock().await;
        s.index("a".into()).await
    });
    tokio::task::yield_now().await;
    let b_waited = {
        let _turn = heavy.lock().await;
        t0.elapsed()
    };
    let out = a.await.unwrap().unwrap();
    assert_eq!((out.indexed, out.skipped), (0, true));
    assert!(
        b_waited <= Duration::from_millis(INDEX_MODEL_WAIT_MS),
        "{b_waited:?}"
    );
    assert!(
        matches!(lazy.state(), EmbedderState::Loading),
        "the download keeps going"
    );
}

/// The meeting skipped while the model downloaded is embedded by the backfill once the model is ready.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n104_a_meeting_skipped_while_the_model_downloads_is_embedded_by_the_backfill() {
    let db = db_with("a", 70);
    let gate = Arc::new(tokio::sync::Notify::new());
    let lazy = gated_loader(gate.clone());
    let mut s = steps(&db, lazy.clone());
    s.model_wait = Duration::from_millis(50);
    let out = s.index("a".into()).await.unwrap();
    assert_eq!((out.indexed, out.skipped), (0, true));
    assert_eq!(vectors(&db, "fake-2d"), 0);
    let backfill = Backfill::new(db.clone(), Arc::new(|_: &str| false));
    let run = tokio::spawn(backfill.clone().run(lazy.clone(), Duration::from_millis(10)));
    gate.notify_one();
    let t0 = std::time::Instant::now();
    while vectors(&db, "fake-2d") < 70 {
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "backfill never embedded the skipped meeting"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    backfill.cancel();
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap();
}

// ------------------------------------------------------------------ N3 (lost with apps/desktop note-service.hardening3)

/// The worker-side deadline of the index step sits inside the runtime's RPC timeout, and the model wait inside
/// the deadline, so a late attempt is always the executor's E_TIMEOUT (retryable, nothing saved late), never the
/// runtime's.
#[test]
fn n3_index_wait_and_deadline_nest_inside_the_rpc_timeout() {
    // Checked at compile time above; this names the behaviour in the test list.
    let [wait, deadline, rpc] =
        std::hint::black_box([INDEX_MODEL_WAIT_MS, INDEX_DEADLINE_MS, INDEX_TIMEOUT_MS]);
    assert!(wait < deadline && deadline < rpc, "{wait} {deadline} {rpc}");
}

// ------------------------------------------------------------------ N105

#[cfg(feature = "minilm")]
mod verify {
    use super::*;
    use qrtz_search::minilm::verify_files;
    use sha2::{Digest, Sha256};

    fn hex(b: &[u8]) -> String {
        format!("{:x}", Sha256::digest(b))
    }

    /// The weights are hashed as they stream: checking a 24 MiB file never makes an allocation near its size
    /// (it used to read the whole file, three times on the boot path).
    #[test]
    fn n105_verify_streams_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let big: Vec<u8> = (0..24 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.path().join("model.safetensors"), &big).unwrap();
        let want = hex(&big);
        drop(big);
        let files = [("model.safetensors", want.as_str())];
        let (r, biggest) = biggest_during(|| verify_files(dir.path(), &files));
        assert_eq!(r, Ok(()));
        assert!(biggest < 1024 * 1024, "largest allocation {biggest} bytes");
    }

    /// Verified once, a file is not hashed again while it is unchanged, but any change in place (same size, mtime
    /// put back) or a replacement is caught on the next check.
    #[test]
    fn n105_a_file_changed_after_it_was_verified_is_caught() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tokenizer.json");
        std::fs::write(&p, b"{\"good\": true}").unwrap();
        let want = hex(b"{\"good\": true}");
        let files = [("tokenizer.json", want.as_str())];
        assert_eq!(verify_files(dir.path(), &files), Ok(()));
        assert_eq!(verify_files(dir.path(), &files), Ok(()), "unchanged: still good");
        let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        {
            use std::io::{Seek, Write};
            let mut f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
            f.seek(std::io::SeekFrom::Start(10)).unwrap();
            f.write_all(b"X").unwrap(); // same length
            f.set_modified(mtime).unwrap();
        }
        let e = verify_files(dir.path(), &files).unwrap_err();
        assert!(e.starts_with("tokenizer.json: sha256 "), "{e}");
        // Replaced by a good copy (new inode): good again.
        let tmp = dir.path().join("t.tmp");
        std::fs::write(&tmp, b"{\"good\": true}").unwrap();
        std::fs::rename(&tmp, &p).unwrap();
        assert_eq!(verify_files(dir.path(), &files), Ok(()));
        std::fs::remove_file(&p).unwrap();
        assert!(verify_files(dir.path(), &files)
            .unwrap_err()
            .starts_with("tokenizer.json: "));
    }
}

// ------------------------------------------------------------------ disk full during the model download

#[cfg(all(feature = "minilm-download", target_os = "linux"))]
mod disk_full {
    use qrtz_search::minilm::{download_files, DownloadLimits};
    use sha2::{Digest, Sha256};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// Serves `body` (with a content-length) for every request; returns the base URL.
    fn serve(body: Vec<u8>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { return };
                let body = body.clone();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(s.try_clone().unwrap());
                    let mut line = String::new();
                    while r.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                        line.clear();
                    }
                    let _ = write!(
                        s,
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = s.write_all(&body);
                });
            }
        });
        base
    }

    /// The `.part` lands on a full disk (a symlink to /dev/full: every write fails with ENOSPC): the download fails
    /// with the OS error, no `.part` and no file under the real name are left, and once space is back the next
    /// attempt succeeds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_full_disk_mid_download_leaves_nothing_and_the_retry_succeeds() {
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 7) as u8).collect();
        let want = format!("{:x}", Sha256::digest(&body));
        let base = serve(body.clone());
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("model.safetensors.part");
        std::os::unix::fs::symlink("/dev/full", &part).unwrap();
        let files = [("model.safetensors", want.as_str())];
        let client = reqwest::Client::new();
        let e = download_files(&client, &base, dir.path(), &files, DownloadLimits::default())
            .await
            .unwrap_err();
        assert!(e.starts_with("model.safetensors: "), "{e}");
        assert!(e.contains("No space left"), "{e}");
        assert!(std::fs::symlink_metadata(&part).is_err(), "no .part left");
        assert!(!dir.path().join("model.safetensors").exists());
        download_files(&client, &base, dir.path(), &files, DownloadLimits::default())
            .await
            .unwrap();
        assert_eq!(std::fs::read(dir.path().join("model.safetensors")).unwrap(), body);
        assert!(std::fs::symlink_metadata(&part).is_err());
    }
}

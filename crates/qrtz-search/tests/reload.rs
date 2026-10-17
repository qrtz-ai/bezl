//! ADR 0064: `LazyEmbedder::reload` puts a model that was downloaded while the app runs into use without a restart.
//! Search stays on full-text while the new model loads and when its files fail the hash check, concurrent reloads
//! never load in parallel, and the backfill embeds for the new model from its next batch.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::InlineDb;
use qrtz_model::*;
use qrtz_search::*;
use qrtz_storage::Db;
use serde_json::json;
use tokio::sync::Notify;

/// Runs inside the first `embed` call.
type Hook = Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

struct Fake {
    id: String,
    calls: AtomicUsize,
    /// Runs inside the first `embed` call (a reload landing while that batch is on the CPU).
    on_first: Mutex<Option<Hook>>,
}

#[async_trait]
impl Embedder for Fake {
    fn id(&self) -> &str {
        &self.id
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let hook = self.on_first.lock().unwrap().take();
        if let Some(h) = hook {
            h().await;
        }
        Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
    }
}

fn fake(id: &str) -> Arc<Fake> {
    Arc::new(Fake {
        id: id.into(),
        calls: AtomicUsize::new(0),
        on_first: Mutex::new(None),
    })
}

/// A loader that answers its n-th call (from 0) with `steps[n]`, or the last one.
type Step = Arc<dyn Fn() -> Result<Arc<dyn Embedder>, String> + Send + Sync>;

fn loader(steps: Vec<Step>, gate: Option<Arc<Notify>>, calls: Arc<AtomicUsize>) -> Arc<LazyEmbedder> {
    LazyEmbedder::new(Box::new(move || {
        let n = calls.fetch_add(1, Ordering::SeqCst);
        let step = steps[n.min(steps.len() - 1)].clone();
        let gate = gate.clone().filter(|_| n > 0);
        Box::pin(async move {
            if let Some(g) = gate {
                g.notified().await;
            }
            step()
        })
    }))
}

fn ok(e: &Arc<Fake>) -> Step {
    let e: Arc<dyn Embedder> = e.clone();
    Arc::new(move || Ok(e.clone()))
}

fn missing() -> Step {
    Arc::new(|| Err("model.safetensors: missing".into()))
}

fn db_with(ms: &[(&str, i64, usize)]) -> InlineDb {
    let db = Db::open_in_memory().unwrap();
    for (id, at, n) in ms {
        let m: Meeting = serde_json::from_value(json!({
            "id": id, "title": format!("T {id}"), "platform": "zoom", "startedAt": at, "status": "done", "participants": []
        }))
        .unwrap();
        db.notes().upsert_meeting(&m).unwrap();
        let segs: Vec<Segment> = (0..*n)
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
    }
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

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..1000 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_stays_on_full_text_while_the_new_model_loads_then_uses_it() {
    let db = db_with(&[("a", 1, 3)]);
    let gate = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let model = fake("fake-new");
    let lazy = loader(vec![missing(), ok(&model)], Some(gate.clone()), calls.clone());
    // At boot MiniLM is not downloaded yet: full-text only.
    assert!(lazy.get().await.is_none());
    let reload = tokio::spawn(lazy.reload());
    until("the reload's load to start", || calls.load(Ordering::SeqCst) == 2).await;
    for _ in 0..3 {
        let r = retrieve(&db, &lazy, "budget", None).await.unwrap();
        assert_eq!(
            (r.candidates.len(), r.ranked.is_none()),
            (3, true),
            "full-text while loading"
        );
        assert!(lazy.ready().is_none());
        assert!(
            lazy.get().await.is_none(),
            "the index step doesn't wait for a reload"
        );
    }
    gate.notify_one();
    assert!(reload.await.unwrap(), "the reload succeeded");
    assert_eq!(lazy.ready().unwrap().id(), "fake-new");
    let r = retrieve(&db, &lazy, "budget", None).await.unwrap();
    assert!(r.ranked.is_some(), "the new model ranks without a restart");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_that_fail_the_hash_check_keep_full_text_search() {
    let db = db_with(&[("a", 1, 2)]);
    let dir = tempfile::tempdir().unwrap();
    for (name, _) in minilm::FILES {
        std::fs::write(dir.path().join(name), b"not the pinned bytes").unwrap();
    }
    let lazy = LazyEmbedder::new(minilm::loader(dir.path().to_path_buf()));
    assert!(lazy.get().await.is_none());
    assert!(
        !lazy.reload().await,
        "a hash mismatch is a failed load, not an error"
    );
    assert!(matches!(lazy.state(), EmbedderState::Unavailable));
    let r = retrieve(&db, &lazy, "budget", None).await.unwrap();
    assert_eq!((r.candidates.len(), r.ranked.is_none()), (2, true));

    // With a model already loaded, a reload whose files are bad keeps that model.
    let calls = Arc::new(AtomicUsize::new(0));
    let old = fake("fake-old");
    let bad: Step = Arc::new(|| Err("model.safetensors: sha256 00 is not the pinned 53aa".into()));
    let lazy = loader(vec![ok(&old), bad], None, calls);
    assert_eq!(lazy.get().await.unwrap().id(), "fake-old");
    assert!(!lazy.reload().await);
    assert_eq!(lazy.ready().unwrap().id(), "fake-old");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_reloads_never_load_in_parallel_and_the_last_ask_is_served() {
    let (live, peak, n) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    );
    let (l2, p2, n2) = (live.clone(), peak.clone(), n.clone());
    let lazy = LazyEmbedder::new(Box::new(move || {
        let (live, peak) = (l2.clone(), p2.clone());
        let k = n2.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let now = live.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(40)).await;
            live.fetch_sub(1, Ordering::SeqCst);
            Ok(fake(&format!("fake-{k}")) as Arc<dyn Embedder>)
        })
    }));
    let (a, b) = (lazy.reload(), lazy.reload());
    let c = {
        let l = lazy.clone();
        tokio::spawn(async move { l.reload().await })
    };
    let (a, b, c) = tokio::join!(a, b, c);
    assert!(a && b && c.unwrap());
    assert_eq!(peak.load(Ordering::SeqCst), 1, "one load at a time");
    // The first ask starts a load; asks made during it share one more load after it.
    let loads = n.load(Ordering::SeqCst);
    assert!((1..=2).contains(&loads), "{loads} loads");
    assert_eq!(
        lazy.ready().unwrap().id(),
        format!("fake-{}", loads - 1),
        "the newest load wins"
    );
    // And a reload racing the first use (start) is still a single load at a time.
    let again = lazy.reload();
    lazy.start();
    assert!(again.await);
    assert_eq!(peak.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_backfill_embeds_for_the_new_model_from_its_next_batch() {
    let db = db_with(&[("a", 1, 300)]);
    let (old, new) = (fake("fake-a"), fake("fake-b"));
    let cell: Arc<OnceLock<Arc<LazyEmbedder>>> = Arc::new(OnceLock::new());
    let c2 = cell.clone();
    // The reload lands while the old model's first batch runs.
    *old.on_first.lock().unwrap() = Some(Box::new(move || {
        let lazy = c2.get().unwrap().clone();
        Box::pin(async move {
            assert!(lazy.reload().await);
        })
    }));
    let lazy = loader(vec![ok(&old), ok(&new)], None, Arc::new(AtomicUsize::new(0)));
    cell.set(lazy.clone()).ok().unwrap();
    let bf = Backfill::new(db.clone(), Arc::new(|_| false));
    let task = tokio::spawn(bf.clone().run(lazy.clone(), Duration::from_secs(30)));
    until("every line embedded for the new model", || {
        vectors(&db, "fake-b") == 300
    })
    .await;
    assert_eq!(
        old.calls.load(Ordering::SeqCst),
        1,
        "the old model ran only the batch it had started"
    );
    // One vector per line: the new model re-embeds every line (64 + 64 + 64 + 64 + 44) and its vectors replace
    // the old batch's.
    assert_eq!(new.calls.load(Ordering::SeqCst), 5);
    assert_eq!(vectors(&db, "fake-a"), 0);
    // A reloadable embedder: the run waits for a next model rather than ending, and the cancel ends it.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!task.is_finished());
    bf.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_backfill_that_had_no_model_at_boot_starts_after_the_reload() {
    let db = db_with(&[("a", 1, 100), ("b", 2, 30)]);
    let model = fake("fake-b");
    let lazy = loader(vec![missing(), ok(&model)], None, Arc::new(AtomicUsize::new(0)));
    let bf = Backfill::new(db.clone(), Arc::new(|_| false));
    let task = tokio::spawn(bf.clone().run(lazy.clone(), Duration::from_secs(30)));
    assert!(lazy.get().await.is_none());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(vectors(&db, "fake-b"), 0);
    assert!(lazy.reload().await);
    until("the backfill to run", || vectors(&db, "fake-b") == 130).await;
    bf.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}

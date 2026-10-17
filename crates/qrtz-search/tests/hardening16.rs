//! Hardening cycle 16: the vector backfill yields to live work (N82, ADR 0055). A busy gate (recording, live
//! dictation, a pipeline step) is asked before every batch; while it is true no new batch starts, a batch already
//! embedded is still saved, and the next pass resumes exactly where this one parked.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::InlineDb;
use qrtz_model::*;
use qrtz_search::*;
use qrtz_storage::Db;
use serde_json::json;

/// Counts calls, records whether any batch *started* while the gate was closed, and closes the gate itself during
/// call number `close_on` (a meeting starting while that batch is on the CPU).
struct Fake {
    calls: AtomicUsize,
    started_while_busy: AtomicUsize,
    busy: Arc<AtomicBool>,
    close_on: Option<usize>,
}

#[async_trait]
impl Embedder for Fake {
    fn id(&self) -> &str {
        "fake-16"
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError> {
        if self.busy.load(Ordering::SeqCst) {
            self.started_while_busy.fetch_add(1, Ordering::SeqCst);
        }
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.close_on == Some(n) {
            self.busy.store(true, Ordering::SeqCst);
        }
        Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
    }
}

fn fake(busy: &Arc<AtomicBool>, close_on: Option<usize>) -> Arc<Fake> {
    Arc::new(Fake {
        calls: AtomicUsize::new(0),
        started_while_busy: AtomicUsize::new(0),
        busy: busy.clone(),
        close_on,
    })
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

fn vectors(db: &InlineDb) -> usize {
    db.0.lock()
        .unwrap()
        .notes()
        .vector_sources("fake-16", None)
        .unwrap()
        .len()
}

fn gated(db: &InlineDb, busy: &Arc<AtomicBool>) -> Arc<Backfill<InlineDb>> {
    let b = busy.clone();
    Backfill::gated(
        db.clone(),
        Arc::new(|_: &str| false),
        Arc::new(move || b.load(Ordering::SeqCst)),
    )
}

#[tokio::test]
async fn a_closed_gate_starts_no_batch() {
    let db = db_with(&[("a", 1, 100)]);
    let busy = Arc::new(AtomicBool::new(true));
    let f = fake(&busy, None);
    let e: Arc<dyn Embedder> = f.clone();
    let r = gated(&db, &busy).pass(&e, BACKFILL_PASS_LINES).await.unwrap();
    assert_eq!(
        (r.embedded, r.paused, r.more, r.cancelled),
        (0, true, true, false)
    );
    assert_eq!((f.calls.load(Ordering::SeqCst), vectors(&db)), (0, 0));
}

#[tokio::test]
async fn a_meeting_that_starts_mid_pass_waits_one_batch_and_that_batch_is_kept() {
    // Two meetings so the gate is seen both inside a meeting (between batches) and between meetings.
    for close_on in 1..=6 {
        let db = db_with(&[("old", 1, 150), ("new", 2, 200)]);
        let busy = Arc::new(AtomicBool::new(false));
        let f = fake(&busy, Some(close_on));
        let e: Arc<dyn Embedder> = f.clone();
        let bf = gated(&db, &busy);
        let r = bf.pass(&e, BACKFILL_PASS_LINES).await.unwrap();
        assert!(r.paused && r.more, "close_on {close_on}: {r:?}");
        assert_eq!(
            f.started_while_busy.load(Ordering::SeqCst),
            0,
            "close_on {close_on}"
        );
        assert_eq!(
            f.calls.load(Ordering::SeqCst),
            close_on,
            "one batch at most after the gate closes"
        );
        // The batch in flight when the gate closed was saved, not thrown away.
        assert_eq!(vectors(&db) as u32, r.embedded, "close_on {close_on}");
        assert!(r.embedded > 0);
        // Gate opens: the next pass resumes and finishes; every line embedded exactly once.
        busy.store(false, Ordering::SeqCst);
        let before = r.embedded;
        let r2 = bf.pass(&e, BACKFILL_PASS_LINES).await.unwrap();
        assert_eq!((r2.paused, r2.more), (false, false), "close_on {close_on}");
        assert_eq!(before + r2.embedded, 350, "close_on {close_on}");
        assert_eq!(vectors(&db), 350);
    }
}

#[tokio::test(start_paused = true)]
async fn run_parks_while_busy_and_finishes_after() {
    let db = db_with(&[("a", 1, 300)]);
    let busy = Arc::new(AtomicBool::new(false));
    let f = fake(&busy, Some(2));
    let e: Arc<dyn Embedder> = f.clone();
    let bf = gated(&db, &busy);
    let task = tokio::spawn(
        bf.clone()
            .run(LazyEmbedder::ready_with(e), Duration::from_secs(30)),
    );
    // A ten-minute meeting: the backfill waits it out, polling the gate, and starts no batch meanwhile.
    for _ in 0..300 {
        tokio::time::sleep(BUSY_POLL).await;
        assert!(!task.is_finished());
    }
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.started_while_busy.load(Ordering::SeqCst), 0);
    busy.store(false, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(60), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vectors(&db), 300);
    assert_eq!(f.calls.load(Ordering::SeqCst), 5); // 64 + 64 | parked | 64 + 64 + 44
}

#[tokio::test(start_paused = true)]
async fn cancel_while_parked_ends_run() {
    let db = db_with(&[("a", 1, 10)]);
    let busy = Arc::new(AtomicBool::new(true));
    let f = fake(&busy, None);
    let e: Arc<dyn Embedder> = f.clone();
    let bf = gated(&db, &busy);
    let task = tokio::spawn(
        bf.clone()
            .run(LazyEmbedder::ready_with(e), Duration::from_secs(30)),
    );
    tokio::time::sleep(BUSY_POLL * 3).await;
    bf.cancel();
    tokio::time::timeout(BUSY_POLL * 2, task).await.unwrap().unwrap();
    assert_eq!((f.calls.load(Ordering::SeqCst), vectors(&db)), (0, 0));
}

#[tokio::test]
async fn the_ungated_constructor_never_parks() {
    let db = db_with(&[("a", 1, 70)]);
    let busy = Arc::new(AtomicBool::new(false));
    let f = fake(&busy, Some(1)); // the flag flips, but `new` has no gate
    let e: Arc<dyn Embedder> = f.clone();
    let r = Backfill::new(db.clone(), Arc::new(|_: &str| false))
        .pass(&e, 1000)
        .await
        .unwrap();
    assert_eq!((r.embedded, r.paused, r.more), (70, false, false));
}

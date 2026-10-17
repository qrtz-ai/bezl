//! Index step, backfill and retrieval with a fake embedder (no model, no network).
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::{CoreSteps, InlineDb, StepExecutor};
use qrtz_model::*;
use qrtz_search::*;
use qrtz_storage::Db;
use serde_json::json;

/// Letter-count vectors, normalized: lines about the same words point the same way.
struct Fake {
    id: String,
    calls: AtomicUsize,
    /// Calls after this many hang forever.
    hang_after: Option<usize>,
    delay: Duration,
}

fn fake(id: &str) -> Arc<Fake> {
    Arc::new(Fake {
        id: id.into(),
        calls: AtomicUsize::new(0),
        hang_after: None,
        delay: Duration::ZERO,
    })
}

#[async_trait]
impl Embedder for Fake {
    fn id(&self) -> &str {
        &self.id
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.hang_after.is_some_and(|h| n >= h) {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(self.delay).await;
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = [0f32; 8];
                for c in t.to_lowercase().chars().filter(|c| c.is_alphabetic()) {
                    v[(c as usize) % 8] += 1.0;
                }
                let s = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
                v.iter().map(|x| x / s).collect()
            })
            .collect())
    }
}

fn meeting(db: &Db, id: &str, started: i64, lines: &[&str]) {
    let m: Meeting = serde_json::from_value(json!({
        "id": id, "title": format!("T {id}"), "platform": "zoom", "startedAt": started, "status": "done", "participants": []
    }))
    .unwrap();
    db.notes().upsert_meeting(&m).unwrap();
    let segs: Vec<Segment> = lines
        .iter()
        .enumerate()
        .map(|(i, t)| {
            serde_json::from_value(
                json!({"id": format!("{id}-{i}"), "startMs": i * 1000, "endMs": i * 1000 + 900,
                "text": t, "speakerName": "Ana", "source": "live"}),
            )
            .unwrap()
        })
        .collect();
    db.notes()
        .replace_segments(id, SegmentSource::Live, &segs)
        .unwrap();
}

fn lines(n: usize, word: &str) -> Vec<String> {
    (0..n).map(|i| format!("{word} line {i}")).collect()
}

fn db_with(ms: &[(&str, i64, Vec<String>)]) -> InlineDb {
    let db = Db::open_in_memory().unwrap();
    for (id, at, ls) in ms {
        meeting(&db, id, *at, &ls.iter().map(String::as_str).collect::<Vec<_>>());
    }
    InlineDb::new(db)
}

fn vectors(db: &InlineDb, model: &str, m: Option<&str>) -> usize {
    db.0.lock()
        .unwrap()
        .notes()
        .vector_sources(model, m)
        .unwrap()
        .len()
}

fn steps(db: &InlineDb, e: Arc<LazyEmbedder>) -> SearchSteps<InlineDb> {
    SearchSteps::new(db.clone(), Arc::new(CoreSteps { db: db.clone() }), e)
}

#[tokio::test]
async fn index_step_embeds_the_meeting_or_skips_without_a_model() {
    let db = db_with(&[("a", 1, lines(150, "budget")), ("b", 2, lines(3, "launch"))]);
    let out = steps(&db, LazyEmbedder::none()).index("a".into()).await.unwrap();
    assert_eq!((out.indexed, out.skipped), (0, true));
    let e = fake("fake-1");
    let out = steps(&db, LazyEmbedder::ready_with(e.clone()))
        .index("a".into())
        .await
        .unwrap();
    assert_eq!((out.indexed, out.skipped), (150, false));
    assert_eq!(e.calls.load(Ordering::SeqCst), 3); // 64 + 64 + 22
    assert_eq!(
        (
            vectors(&db, "fake-1", Some("a")),
            vectors(&db, "fake-1", Some("b"))
        ),
        (150, 0)
    );
    let e = steps(&db, LazyEmbedder::none())
        .index("nope".into())
        .await
        .unwrap_err();
    assert_eq!(e.code, "E_NOT_FOUND");
}

#[tokio::test(start_paused = true)]
async fn index_deadline_keeps_saved_batches_and_the_retry_resumes() {
    let db = db_with(&[("a", 1, lines(200, "budget"))]);
    let hang = Arc::new(Fake {
        hang_after: Some(2),
        ..Arc::into_inner(fake("fake-1")).unwrap()
    });
    let e = steps(&db, LazyEmbedder::ready_with(hang))
        .index("a".into())
        .await
        .unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.retryable, e.message.as_str()),
        ("E_TIMEOUT", true, "index deadline exceeded")
    );
    assert_eq!(vectors(&db, "fake-1", None), 128);
    let out = steps(&db, LazyEmbedder::ready_with(fake("fake-1")))
        .index("a".into())
        .await
        .unwrap();
    assert_eq!(out.indexed, 72);
}

/// N104 (ADR 0068): a model that never finishes loading is waited for `INDEX_MODEL_WAIT_MS`, not the whole
/// deadline; the step skips (the backfill embeds the meeting later) and the load keeps going.
#[tokio::test(start_paused = true)]
async fn a_model_that_never_loads_is_skipped_after_the_model_wait() {
    let db = db_with(&[("a", 1, lines(3, "budget"))]);
    let never = LazyEmbedder::new(Box::new(|| Box::pin(std::future::pending())));
    let t0 = tokio::time::Instant::now();
    let out = steps(&db, never.clone()).index("a".into()).await.unwrap();
    assert_eq!((out.indexed, out.skipped), (0, true));
    assert_eq!(t0.elapsed(), Duration::from_millis(INDEX_MODEL_WAIT_MS));
    assert!(matches!(never.state(), EmbedderState::Loading));
}

#[tokio::test]
async fn backfill_is_bounded_resumable_skips_tombstones_and_reembeds_on_a_model_change() {
    let db = db_with(&[
        ("old", 1, lines(100, "budget")),
        ("new", 3, lines(30, "launch")),
        ("gone", 2, lines(10, "x")),
    ]);
    let tomb = Arc::new(Mutex::new(vec!["gone".to_string()]));
    let t2 = tomb.clone();
    let bf = Backfill::new(
        db.clone(),
        Arc::new(move |id: &str| t2.lock().unwrap().iter().any(|t| t == id)),
    );
    let e: Arc<dyn Embedder> = fake("m-1");
    let r = bf.pass(&e, 50).await.unwrap();
    // Newest meeting first, bounded: all 30 of "new", then 20 of "old".
    assert_eq!((r.embedded, r.more), (50, true));
    assert_eq!(
        (vectors(&db, "m-1", Some("new")), vectors(&db, "m-1", Some("old"))),
        (30, 20)
    );
    let r = bf.pass(&e, 1000).await.unwrap();
    assert_eq!((r.embedded, r.more), (80, false));
    assert_eq!(vectors(&db, "m-1", Some("gone")), 0);
    // A new model id: every line is missing again, and its vectors replace the old ones.
    let e2: Arc<dyn Embedder> = fake("m-2");
    let r = bf.pass(&e2, 1000).await.unwrap();
    assert_eq!(r.embedded, 130);
    assert_eq!((vectors(&db, "m-1", None), vectors(&db, "m-2", None)), (0, 130));
    bf.cancel();
    tomb.lock().unwrap().clear();
    let e3: Arc<dyn Embedder> = fake("m-3");
    let r = bf.pass(&e3, 1000).await.unwrap();
    assert_eq!((r.embedded, r.cancelled, r.more), (0, true, true));
}

#[tokio::test]
async fn a_vector_for_a_line_that_changed_meanwhile_is_dropped() {
    let db = db_with(&[("a", 1, lines(3, "budget"))]);
    let todo =
        db.0.lock()
            .unwrap()
            .notes()
            .missing_vectors("m", 10, Some("a"))
            .unwrap();
    // Re-finalized while embedding: the lines are replaced (new rowids, maybe the same numbers).
    meeting(
        &db.0.lock().unwrap(),
        "a",
        1,
        &["something else", "budget line 1"],
    );
    let rows: Vec<_> = todo
        .iter()
        .map(|t| {
            (
                qrtz_storage::VectorRow {
                    rowid: t.rowid,
                    meeting_id: t.meeting_id.clone(),
                    vec: vec_to_blob(&[1.0]),
                },
                t.text.clone(),
            )
        })
        .collect();
    let n =
        db.0.lock()
            .unwrap()
            .notes()
            .save_vectors_if_current("m", &rows)
            .unwrap();
    let left: Vec<String> =
        db.0.lock()
            .unwrap()
            .notes()
            .vector_sources("m", None)
            .unwrap()
            .into_iter()
            .map(|v| v.text)
            .collect();
    assert!(left.iter().all(|t| t == "budget line 1"), "{left:?}");
    assert!(n <= 1);
}

#[tokio::test(start_paused = true)]
async fn retrieval_never_waits_for_the_model() {
    let db = db_with(&[(
        "a",
        1,
        vec!["the budget is approved".into(), "launch on friday".into()],
    )]);
    let (tx, rx) = tokio::sync::oneshot::channel::<Arc<dyn Embedder>>();
    let rx = Mutex::new(Some(rx));
    let lazy = LazyEmbedder::new(Box::new(move || {
        let rx = rx.lock().unwrap().take().unwrap();
        Box::pin(async move { rx.await.map_err(|e| e.to_string()) })
    }));
    // Loading: full-text only, at once.
    let r = retrieve(&db, &lazy, "budget", None).await.unwrap();
    assert_eq!(r.candidates.len(), 1);
    assert!(r.ranked.is_none());
    let e = fake("fake-1");
    tx.send(e.clone()).ok();
    lazy.get().await.unwrap();
    steps(&db, lazy.clone()).index("a".into()).await.unwrap();
    let r = retrieve(&db, &lazy, "budget", None).await.unwrap();
    let ranked = r.ranked.unwrap();
    assert_eq!(ranked[0].text, "the budget is approved");
    // One meeting: its lines are the candidates.
    let r = retrieve(&db, &lazy, "friday", Some("a".into())).await.unwrap();
    assert_eq!(r.candidates.len(), 2);
    assert_eq!(r.ranked.unwrap()[0].text, "launch on friday");
}

#[tokio::test(start_paused = true)]
async fn a_slow_question_embed_falls_back_to_full_text() {
    let db = db_with(&[("a", 1, vec!["the budget is approved".into()])]);
    let slow = Arc::new(Fake {
        delay: Duration::from_secs(5),
        ..Arc::into_inner(fake("fake-1")).unwrap()
    });
    let r = retrieve(&db, &LazyEmbedder::ready_with(slow), "budget", None)
        .await
        .unwrap();
    assert_eq!((r.candidates.len(), r.ranked), (1, None));
}

#[tokio::test]
async fn a_model_that_fails_to_load_means_full_text_only() {
    let db = db_with(&[("a", 1, vec!["the budget".into()])]);
    let bad = LazyEmbedder::new(Box::new(|| Box::pin(async { Err("no weights".to_string()) })));
    assert!(bad.get().await.is_none());
    assert!(matches!(bad.state(), EmbedderState::Unavailable));
    let out = steps(&db, bad.clone()).index("a".into()).await.unwrap();
    assert!(out.skipped);
    assert!(retrieve(&db, &bad, "budget", None)
        .await
        .unwrap()
        .ranked
        .is_none());
}

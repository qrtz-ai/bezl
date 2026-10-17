//! Hardening cycle 15: chat retrieval keeps only what it ranks (N80) and a NaN vector never panics it (N79).
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::{CoreSteps, InlineDb, StepExecutor};
use qrtz_model::*;
use qrtz_search::*;
use qrtz_storage::{Db, VectorRow};
use serde_json::json;

/// Letter-count vectors, normalized; `nan_for` lines get NaN (a zero-norm line in a model that divides by the norm).
struct Fake {
    nan_for: Option<String>,
}

fn vec_of(t: &str) -> Vec<f32> {
    let mut v = [0f32; 8];
    for c in t.to_lowercase().chars().filter(|c| c.is_alphabetic()) {
        v[(c as usize) % 8] += 1.0;
    }
    let s = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter().map(|x| x / s).collect()
}

#[async_trait]
impl Embedder for Fake {
    fn id(&self) -> &str {
        "fake-1"
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError> {
        Ok(texts
            .iter()
            .map(|t| {
                if self.nan_for.as_deref() == Some(t.as_str()) {
                    vec![f32::NAN; 8]
                } else {
                    vec_of(t)
                }
            })
            .collect())
    }
}

fn meeting(db: &Db, id: &str, started: i64, lines: &[String]) {
    let m: Meeting = serde_json::from_value(json!({
        "id": id, "title": format!("T {id}"), "platform": "zoom", "startedAt": started, "status": "done", "participants": []
    }))
    .unwrap();
    db.notes().upsert_meeting(&m).unwrap();
    let segs: Vec<Segment> = lines
        .iter()
        .enumerate()
        .map(|(i, t)| {
            serde_json::from_value(json!({"id": format!("{id}-{i}"), "startMs": (i % 7) * 1000, "endMs": (i % 7) * 1000 + 900,
                "text": t, "speakerName": if i % 3 == 0 { "" } else { "Ana" }, "speaker": "S1", "source": "live"}))
            .unwrap()
        })
        .collect();
    db.notes()
        .replace_segments(id, SegmentSource::Live, &segs)
        .unwrap();
}

/// Deterministic pseudo-random lines from a small vocabulary: many equal vectors, so ties at the cut are common.
fn library(meetings: usize, per: usize, seed: u64) -> InlineDb {
    const W: [&str; 12] = [
        "budget", "launch", "friday", "hiring", "roadmap", "pricing", "deadline", "design", "review", "bug",
        "customer", "demo",
    ];
    let mut x = seed;
    let mut next = || {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (x >> 33) as usize
    };
    let db = Db::open_in_memory().unwrap();
    for m in 0..meetings {
        let ls: Vec<String> = (0..per)
            .map(|_| {
                let n = 1 + next() % 3;
                (0..n).map(|_| W[next() % W.len()]).collect::<Vec<_>>().join(" ")
            })
            .collect();
        meeting(&db, &format!("m{m}"), m as i64, &ls);
    }
    InlineDb::new(db)
}

async fn index_all(db: &InlineDb, e: Arc<LazyEmbedder>, ids: impl Iterator<Item = String>) {
    let s = SearchSteps::new(db.clone(), Arc::new(CoreSteps { db: db.clone() }), e);
    for id in ids {
        s.index(id).await.unwrap();
    }
}

type Key = (String, i64, String);
fn key(s: &ChatSource) -> Key {
    (s.meeting_id.clone(), s.start_ms, s.text.clone())
}

/// The pre-N80 algorithm (every vector collected, then a stable sort): the oracle for the streamed pass.
fn reference(
    db: &InlineDb,
    question: &str,
    candidates: &[ChatSource],
    meeting_id: Option<&str>,
) -> Vec<ChatSource> {
    let qvec = vec_of(question);
    let indexed: Vec<VecSource> =
        db.0.lock()
            .unwrap()
            .notes()
            .vector_sources("fake-1", meeting_id)
            .unwrap()
            .into_iter()
            .map(|r| VecSource {
                vec: Some(blob_to_vec(&r.vec)),
                source: ChatSource {
                    meeting_id: r.meeting_id,
                    title: r.title,
                    started_at: r.started_at,
                    start_ms: r.start_ms,
                    speaker: r.speaker,
                    text: r.text,
                },
            })
            .collect();
    let by_key: HashMap<_, &VecSource> = indexed.iter().map(|s| (key(&s.source), s)).collect();
    let mut pool: Vec<VecSource> = candidates
        .iter()
        .map(|c| {
            by_key.get(&key(c)).map(|s| (*s).clone()).unwrap_or(VecSource {
                source: c.clone(),
                vec: None,
            })
        })
        .collect();
    if meeting_id.is_none() {
        let seen: HashSet<_> = pool.iter().map(|s| key(&s.source)).collect();
        let mut extra: Vec<(&VecSource, f64)> = indexed
            .iter()
            .filter(|s| !seen.contains(&key(&s.source)))
            .map(|s| (s, dot(&qvec, s.vec.as_deref().unwrap_or(&[]))))
            .collect();
        extra.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        pool.extend(extra.into_iter().take(40).map(|x| x.0.clone()));
    }
    hybrid_rank(question, &pool, Some(&qvec), 24, 0.25)
}

#[tokio::test]
async fn n80_streamed_retrieval_ranks_exactly_like_collecting_every_vector() {
    let questions = [
        "budget",
        "when is the launch",
        "pricing review friday",
        "customer demo bug",
        "zzz nothing matches",
        "design",
    ];
    let mut checked = 0;
    for seed in 1..=4u64 {
        let db = library(6, 120, seed);
        let e = LazyEmbedder::ready_with(Arc::new(Fake { nan_for: None }));
        index_all(&db, e.clone(), (0..6).map(|m| format!("m{m}"))).await;
        for q in questions {
            for m in [None, Some("m2")] {
                let r = retrieve(&db, &e, q, m.map(String::from)).await.unwrap();
                let want = reference(&db, q, &r.candidates, m);
                assert_eq!(r.ranked.unwrap(), want, "seed {seed} q {q:?} meeting {m:?}");
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 48);
}

#[tokio::test]
async fn n79_a_nan_vector_in_the_db_does_not_panic_chat() {
    let ls: Vec<String> = ["alpha one", "beta two", "gamma three", "delta four"]
        .map(String::from)
        .to_vec();
    let db = library(2, 30, 9);
    meeting(&db.0.lock().unwrap(), "x", 99, &ls);
    let e = LazyEmbedder::ready_with(Arc::new(Fake { nan_for: None }));
    index_all(&db, e.clone(), ["m0", "m1", "x"].map(String::from).into_iter()).await;
    // A NaN vector written by an older build or another model run, as found in an imported backup.
    {
        let g = db.0.lock().unwrap();
        let rows = g.notes().missing_vectors("fake-1", 10, Some("x")).unwrap();
        assert!(rows.is_empty());
        let rowid: i64 = g
            .connection()
            .query_row(
                "SELECT rowid FROM segments WHERE meeting_id = 'x' AND text = 'beta two'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let nan = vec_to_blob(&[f32::NAN; 8]);
        let n = g
            .notes()
            .save_vectors_if_current(
                "fake-1",
                &[(
                    VectorRow {
                        rowid,
                        meeting_id: "x".into(),
                        vec: nan,
                    },
                    "beta two".into(),
                )],
            )
            .unwrap();
        assert_eq!(n, 1);
    }
    // "budget" never matches "beta two" by full text, so the NaN line goes through the similarity cut.
    let r = retrieve(&db, &e, "budget", None).await.unwrap();
    let ranked = r.ranked.expect("hybrid ranking ran");
    assert!(!ranked.is_empty());
    assert!(
        ranked.iter().all(|s| s.text != "beta two"),
        "a NaN line is never similar"
    );
}

#[tokio::test]
async fn n79_a_non_finite_embedding_is_stored_as_zero() {
    let ls: Vec<String> = ["budget approved", "launch friday"].map(String::from).to_vec();
    let db = InlineDb::new(Db::open_in_memory().unwrap());
    meeting(&db.0.lock().unwrap(), "a", 1, &ls);
    let e = LazyEmbedder::ready_with(Arc::new(Fake {
        nan_for: Some("launch friday".into()),
    }));
    index_all(&db, e.clone(), std::iter::once("a".to_string())).await;
    let rows =
        db.0.lock()
            .unwrap()
            .notes()
            .vector_sources("fake-1", Some("a"))
            .unwrap();
    assert_eq!(
        rows.len(),
        2,
        "the line is indexed, so the backfill does not pick it again"
    );
    let v = blob_to_vec(&rows.iter().find(|r| r.text == "launch friday").unwrap().vec);
    assert!(v.iter().all(|x| *x == 0.0), "{v:?}");
}

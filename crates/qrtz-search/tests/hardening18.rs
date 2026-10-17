//! Hardening cycle 18 (N90, ADR 0060): chat never sees a meeting whose delete is pending (undo window), neither as
//! a full-text candidate, nor as a "closest line" from the vectors, nor when asked about that meeting directly.
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::{CoreSteps, InlineDb, StepExecutor};
use qrtz_model::*;
use qrtz_search::*;
use qrtz_storage::Db;
use serde_json::json;

/// Letter-count vectors, normalized (as in hardening15).
struct Fake;

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
        Ok(texts.iter().map(|t| vec_of(t)).collect())
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
                "text": t, "speaker": "S1", "source": "live"}),
            )
            .unwrap()
        })
        .collect();
    db.notes()
        .replace_segments(id, SegmentSource::Live, &segs)
        .unwrap();
}

fn hide(ids: &[&str]) -> (HiddenFn, Arc<AtomicUsize>) {
    let set: HashSet<String> = ids.iter().map(|s| s.to_string()).collect();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    (
        Arc::new(move |id: &str| {
            c.fetch_add(1, Ordering::Relaxed);
            set.contains(id)
        }),
        calls,
    )
}

fn ids(v: &[ChatSource]) -> HashSet<String> {
    v.iter().map(|s| s.meeting_id.clone()).collect()
}

async fn index_all(db: &InlineDb, e: Arc<LazyEmbedder>, ids: &[&str]) {
    let s = SearchSteps::new(db.clone(), Arc::new(CoreSteps { db: db.clone() }), e);
    for id in ids {
        s.index(id.to_string()).await.unwrap();
    }
}

#[tokio::test]
async fn n90_full_text_candidates_skip_a_meeting_pending_delete() {
    let db = InlineDb::new(Db::open_in_memory().unwrap());
    {
        let g = db.0.lock().unwrap();
        meeting(&g, "gone", 1, &["the budget is secret", "budget layoffs friday"]);
        meeting(&g, "keep", 2, &["budget review on monday"]);
    }
    let none = LazyEmbedder::none();
    let all = retrieve(&db, &none, "budget", None).await.unwrap();
    assert_eq!(ids(&all.candidates), ["gone", "keep"].map(String::from).into());
    let (h, _) = hide(&["gone"]);
    let r = retrieve_visible(&db, &none, "budget", None, Some(h))
        .await
        .unwrap();
    assert_eq!(ids(&r.candidates), ["keep"].map(String::from).into());
    assert!(r.ranked.is_none());
    let src = r.into_sources("budget");
    assert!(src.iter().all(|s| s.meeting_id == "keep"));
}

#[tokio::test]
async fn n90_hidden_lines_do_not_crowd_out_the_full_text_slots() {
    // 120 matching lines in the pending meeting would take every one of the 80 slots.
    let db = InlineDb::new(Db::open_in_memory().unwrap());
    {
        let g = db.0.lock().unwrap();
        let many: Vec<String> = (0..120).map(|i| format!("budget line {i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        meeting(&g, "gone", 1, &many);
        let few: Vec<String> = (0..30)
            .map(|i| format!("the budget plan number {i} for the next quarter"))
            .collect();
        let few: Vec<&str> = few.iter().map(String::as_str).collect();
        meeting(&g, "keep", 2, &few);
    }
    let none = LazyEmbedder::none();
    let (h, _) = hide(&["gone"]);
    let r = retrieve_visible(&db, &none, "budget", None, Some(h))
        .await
        .unwrap();
    assert_eq!(r.candidates.len(), 30, "every visible match is still found");
    assert!(r.candidates.iter().all(|s| s.meeting_id == "keep"));
}

#[tokio::test]
async fn n90_closest_lines_from_vectors_skip_a_meeting_pending_delete() {
    let db = InlineDb::new(Db::open_in_memory().unwrap());
    {
        let g = db.0.lock().unwrap();
        // Similar letters to the question but no full-text match: only the vector pass can find these.
        meeting(&g, "gone", 1, &["tegdub", "gudbet", "budgets"]);
        meeting(&g, "keep", 2, &["butted", "debug"]);
    }
    let e = LazyEmbedder::ready_with(Arc::new(Fake));
    index_all(&db, e.clone(), &["gone", "keep"]).await;
    let all = retrieve(&db, &e, "budget", None).await.unwrap();
    assert!(
        ids(all.ranked.as_ref().unwrap()).contains("gone"),
        "precondition: the vector pass finds it"
    );
    let (h, calls) = hide(&["gone"]);
    let r = retrieve_visible(&db, &e, "budget", None, Some(h)).await.unwrap();
    let ranked = r.ranked.clone().expect("hybrid ranking ran");
    assert!(!ranked.is_empty());
    assert!(ranked.iter().all(|s| s.meeting_id == "keep"), "{ranked:?}");
    assert!(r.candidates.iter().all(|s| s.meeting_id == "keep"));
    // Asked once per meeting per pass, not once per line.
    assert!(
        calls.load(Ordering::Relaxed) <= 4,
        "{}",
        calls.load(Ordering::Relaxed)
    );
}

#[tokio::test]
async fn n90_a_question_about_a_meeting_pending_delete_gets_no_lines() {
    let db = InlineDb::new(Db::open_in_memory().unwrap());
    {
        let g = db.0.lock().unwrap();
        meeting(&g, "gone", 1, &["the budget is secret"]);
    }
    for e in [LazyEmbedder::none(), LazyEmbedder::ready_with(Arc::new(Fake))] {
        index_all(&db, e.clone(), &["gone"]).await;
        let (h, _) = hide(&["gone"]);
        let r = retrieve_visible(&db, &e, "what is the budget", Some("gone".into()), Some(h))
            .await
            .unwrap();
        assert!(r.candidates.is_empty());
        assert!(r.into_sources("what is the budget").is_empty());
    }
    // Nothing hidden: the same call is unchanged.
    let r = retrieve_visible(&db, &LazyEmbedder::none(), "budget", Some("gone".into()), None)
        .await
        .unwrap();
    assert_eq!(r.candidates.len(), 1);
}

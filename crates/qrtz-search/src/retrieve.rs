//! Chat retrieval (the retrieval half of the worker's `note.chat`): full-text candidates (`search_segments`, with
//! N47/N49 splitting) or one meeting's lines, then, when a model is already loaded, hybrid ranking with the lines'
//! stored vectors plus the semantically closest lines FTS missed. It never waits for the embedder: before the model
//! is ready, or if the question embed takes longer than [`QUESTION_EMBED_TIMEOUT_MS`], it is full-text only. It
//! does not embed missing lines inline (TS did up to 2,000); the backfill does.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use qrtz_lifecycle::runtime::{lifecycle_error, DbExec};
use qrtz_model::LifecycleError;

use crate::embedder::{blob_to_vec, dot, Embedder, LazyEmbedder};
use crate::rank::{hybrid_rank, ChatSource, VecSource};

pub const QUESTION_EMBED_TIMEOUT_MS: u64 = 1_500;
/// Candidates from full-text search across meetings (TS: 80).
pub const FTS_CANDIDATES: u32 = 80;

/// What the answerer gets: the candidates, and `ranked` when hybrid ranking ran (`preRanked`); without it the
/// answerer ranks lexically (`rankSources`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retrieval {
    pub candidates: Vec<ChatSource>,
    pub ranked: Option<Vec<ChatSource>>,
}

fn key(s: &ChatSource) -> (String, i64, String) {
    (s.meeting_id.clone(), s.start_ms, s.text.clone())
}

pub async fn retrieve<E: DbExec>(
    db: &E,
    embedder: &Arc<LazyEmbedder>,
    question: &str,
    meeting_id: Option<String>,
) -> Result<Retrieval, LifecycleError> {
    let (q, m) = (question.to_string(), meeting_id.clone());
    let candidates = db
        .run(move |db| match m {
            Some(id) => {
                let mt = db.notes().need(&id)?;
                Ok(db
                    .notes()
                    .get_segments(&id)?
                    .into_iter()
                    .map(|s| ChatSource {
                        meeting_id: mt.id.clone(),
                        title: mt.title.clone(),
                        started_at: mt.started_at,
                        start_ms: s.start_ms,
                        speaker: s.speaker_name.or(s.speaker).filter(|x| !x.is_empty()),
                        text: s.text,
                    })
                    .collect())
            }
            None => db.notes().search_segments(&q, FTS_CANDIDATES),
        })
        .await
        .map_err(lifecycle_error)?;
    let ranked = match embedder.ready() {
        Some(e) => match semantic(db, &e, question, &candidates, meeting_id).await {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(code = %err.code, err = %err.message, "semantic retrieval failed; using full-text");
                None
            }
        },
        None => None,
    };
    Ok(Retrieval { candidates, ranked })
}

async fn semantic<E: DbExec>(
    db: &E,
    e: &Arc<dyn Embedder>,
    question: &str,
    candidates: &[ChatSource],
    meeting_id: Option<String>,
) -> Result<Option<Vec<ChatSource>>, LifecycleError> {
    let q = [question.to_string()];
    let embed = e.embed(&q);
    let Ok(v) = tokio::time::timeout(Duration::from_millis(QUESTION_EMBED_TIMEOUT_MS), embed).await else {
        return Ok(None);
    };
    let Some(qvec) = v?.into_iter().next() else {
        return Ok(None);
    };
    let (model, across) = (e.id().to_string(), meeting_id.is_none());
    let indexed = db
        .run(move |db| db.notes().vector_sources(&model, meeting_id.as_deref()))
        .await
        .map_err(lifecycle_error)?;
    let indexed: Vec<VecSource> = indexed
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
    if across {
        // Across meetings: add the semantically closest lines full-text search missed.
        let seen: HashSet<_> = pool.iter().map(|s| key(&s.source)).collect();
        let mut extra: Vec<(&VecSource, f64)> = indexed
            .iter()
            .filter(|s| !seen.contains(&key(&s.source)))
            .map(|s| (s, dot(&qvec, s.vec.as_deref().unwrap_or(&[]))))
            .collect();
        extra.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        pool.extend(extra.into_iter().take(40).map(|x| x.0.clone()));
    }
    Ok(Some(hybrid_rank(question, &pool, Some(&qvec), 24, 0.25)))
}

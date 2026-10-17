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

type Key = (String, i64, String);

fn key(s: &ChatSource) -> Key {
    (s.meeting_id.clone(), s.start_ms, s.text.clone())
}

/// Lines outside the full-text candidates added by similarity, across meetings (TS: 40).
pub const EXTRA_LINES: usize = 40;

/// A similarity that sorts: NaN (a corrupt or foreign vector) ranks below everything, never panics (N79).
fn sim_key(s: f64) -> f64 {
    if s.is_nan() {
        f64::NEG_INFINITY
    } else {
        s
    }
}

/// The best `k` by similarity, first offered first among equals (what a stable descending sort then
/// `take(k)` gives), holding at most `k` items. Ties at the cut keep the earlier one.
pub(crate) struct TopK<T> {
    k: usize,
    items: Vec<(f64, T)>,
}

impl<T> TopK<T> {
    pub(crate) fn new(k: usize) -> Self {
        TopK {
            k,
            items: Vec::with_capacity(k + 1),
        }
    }
    /// `make` runs only when the item is kept.
    pub(crate) fn offer(&mut self, sim: f64, make: impl FnOnce() -> T) {
        let s = sim_key(sim);
        if self.k == 0 {
            return;
        }
        if self.items.len() == self.k && self.items.last().is_some_and(|w| s <= w.0) {
            return;
        }
        // Descending by similarity, ascending by arrival: the new item goes after every item it does not beat.
        let at = self.items.partition_point(|x| x.0 >= s);
        self.items.insert(at, (s, make()));
        self.items.truncate(self.k);
    }
    pub(crate) fn into_sorted(self) -> Vec<T> {
        self.items.into_iter().map(|x| x.1).collect()
    }
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
    let wanted: HashSet<Key> = candidates.iter().map(key).collect();
    let qv = qvec.clone();
    // One streamed pass over the stored vectors (N80, ADR 0052): only the candidates' vectors and the best
    // [`EXTRA_LINES`] other lines are kept, not every indexed line of the library.
    let (found, extra) = db
        .run(move |db| {
            let mut found: HashMap<Key, VecSource> = HashMap::new();
            let mut extra = TopK::new(if across { EXTRA_LINES } else { 0 });
            db.notes()
                .for_each_vector_source(&model, meeting_id.as_deref(), |r| {
                    let src = ChatSource {
                        meeting_id: r.meeting_id,
                        title: r.title,
                        started_at: r.started_at,
                        start_ms: r.start_ms,
                        speaker: r.speaker,
                        text: r.text,
                    };
                    let k = key(&src);
                    if wanted.contains(&k) {
                        // The indexed row stands in for the candidate, last row winning, as the `by_key` map did.
                        found.insert(
                            k,
                            VecSource {
                                vec: Some(blob_to_vec(&r.vec)),
                                source: src,
                            },
                        );
                    } else if across {
                        let v = blob_to_vec(&r.vec);
                        let sim = dot(&qv, &v);
                        extra.offer(sim, || VecSource {
                            vec: Some(v),
                            source: src,
                        });
                    }
                    true
                })?;
            Ok((found, extra.into_sorted()))
        })
        .await
        .map_err(lifecycle_error)?;
    let mut pool: Vec<VecSource> = candidates
        .iter()
        .map(|c| {
            found.get(&key(c)).cloned().unwrap_or(VecSource {
                source: c.clone(),
                vec: None,
            })
        })
        .collect();
    // Across meetings: the semantically closest lines full-text search missed.
    pool.extend(extra);
    Ok(Some(hybrid_rank(question, &pool, Some(&qvec), 24, 0.25)))
}

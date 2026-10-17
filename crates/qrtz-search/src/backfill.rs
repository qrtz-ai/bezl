//! Lazy indexing's other half: once a model is ready, embed the lines that still have no vector for it (meetings
//! indexed while no model was loaded, or every line after a model change, since the model id is stored with each
//! vector). Bounded (at most `max_lines` per pass, in [`crate::index::BATCH`]-line batches with a yield between
//! them), resumable (the next pass starts from what is still missing) and cancelable: [`Backfill::cancel`] stops it
//! before the next batch, and a deleted or tombstoned meeting is skipped and its in-flight vectors dropped.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use qrtz_lifecycle::runtime::{lifecycle_error, DbExec};
use qrtz_model::LifecycleError;

use crate::embedder::{Embedder, LazyEmbedder};
use crate::index::{index_vectors, StopFn};

/// Lines one backfill pass embeds at most.
pub const BACKFILL_PASS_LINES: u32 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PassReport {
    pub embedded: u32,
    /// Lines may still be missing (the pass hit its bound, or was cancelled).
    pub more: bool,
    pub cancelled: bool,
}

pub struct Backfill<E: DbExec> {
    db: E,
    cancelled: Arc<AtomicBool>,
    /// `PendingDeletes::has` in the app: a meeting in its undo window is not embedded.
    tombstoned: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

impl<E: DbExec + Clone> Backfill<E> {
    pub fn new(db: E, tombstoned: Arc<dyn Fn(&str) -> bool + Send + Sync>) -> Arc<Self> {
        Arc::new(Backfill {
            db,
            cancelled: Arc::new(AtomicBool::new(false)),
            tombstoned,
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// One bounded pass over the meetings that miss vectors for `e`, newest meeting first.
    pub async fn pass(&self, e: &Arc<dyn Embedder>, max_lines: u32) -> Result<PassReport, LifecycleError> {
        let model = e.id().to_string();
        let ids = self
            .db
            .run(move |db| db.notes().meetings_missing_vectors(&model))
            .await
            .map_err(lifecycle_error)?;
        let (cancelled, tomb) = (self.cancelled.clone(), self.tombstoned.clone());
        let stop: StopFn = Arc::new(move |id: &str| cancelled.load(Ordering::SeqCst) || tomb(id));
        let mut r = PassReport::default();
        for id in ids {
            if self.is_cancelled() {
                r.cancelled = true;
                r.more = true;
                return Ok(r);
            }
            if (self.tombstoned)(&id) {
                continue;
            }
            if r.embedded >= max_lines {
                r.more = true;
                return Ok(r);
            }
            r.embedded += index_vectors(&self.db, e, max_lines - r.embedded, Some(id), Some(&stop)).await?;
            tokio::task::yield_now().await;
        }
        r.cancelled = self.is_cancelled();
        // At the bound the last meeting may have more lines left; the next pass finds out.
        r.more = r.cancelled || r.embedded >= max_lines;
        Ok(r)
    }

    /// Waits for the model, then runs passes until nothing is missing or it is cancelled. A failed pass (busy DB,
    /// embedder error) is retried after `retry_after`. Spawn it at boot.
    pub async fn run(self: Arc<Self>, embedder: Arc<LazyEmbedder>, retry_after: std::time::Duration) {
        let e = embedder.when_ready().await;
        while !self.is_cancelled() {
            match self.pass(&e, BACKFILL_PASS_LINES).await {
                Ok(r) if !r.more => return,
                Ok(_) => tokio::task::yield_now().await,
                Err(err) => {
                    tracing::warn!(code = %err.code, err = %err.message, "vector backfill pass failed");
                    tokio::time::sleep(retry_after).await;
                }
            }
        }
    }
}

//! Lazy indexing's other half: once a model is ready, embed the lines that still have no vector for it (meetings
//! indexed while no model was loaded, or every line after a model change, since the model id is stored with each
//! vector). Bounded (at most `max_lines` per pass, in [`crate::index::BATCH`]-line batches with a yield between
//! them), resumable (the next pass starts from what is still missing) and cancelable: [`Backfill::cancel`] stops it
//! before the next batch, and a deleted or tombstoned meeting is skipped and its in-flight vectors dropped.
//!
//! It also yields to live work (N82, ADR 0055): a `busy()` gate (recording, live dictation, a summary or index step
//! in the app) is asked before every batch. While it is true the pass parks (`PassReport::paused`) and [`Backfill::run`]
//! waits [`BUSY_POLL`] before asking again, so at most one [`crate::index::BATCH`]-line batch that had already started
//! runs alongside on-device ASR.
//!
//! It follows [`LazyEmbedder::reload`] (ADR 0064): a model that replaced the one a pass started with stops that pass
//! before its next batch (`PassReport::superseded`) and the next pass embeds for the new model; with a reloadable
//! embedder [`Backfill::run`] does not end when nothing is missing but waits for a new model (or the cancel).
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use qrtz_lifecycle::runtime::{lifecycle_error, DbExec};
use qrtz_model::LifecycleError;

use crate::embedder::{same_embedder, Embedder, LazyEmbedder};
use crate::index::{index_vectors_paused, StopFn};

/// Lines one backfill pass embeds at most.
pub const BACKFILL_PASS_LINES: u32 = 2_000;

/// How long [`Backfill::run`] waits before asking a busy gate again.
pub const BUSY_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// Is live work running that the backfill must not compete with (recording, dictation, a pipeline step)?
pub type BusyFn = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PassReport {
    pub embedded: u32,
    /// Lines may still be missing (the pass hit its bound, or was cancelled).
    pub more: bool,
    pub cancelled: bool,
    /// The pass stopped because the busy gate said so; nothing was lost, the next pass resumes.
    pub paused: bool,
    /// ADR 0064: the pass stopped because a reload replaced its model; the next pass embeds for the new one.
    pub superseded: bool,
}

pub struct Backfill<E: DbExec> {
    db: E,
    cancelled: Arc<AtomicBool>,
    /// Wakes a [`Backfill::run`] waiting for a model.
    cancel_tx: tokio::sync::watch::Sender<bool>,
    /// `PendingDeletes::has` in the app: a meeting in its undo window is not embedded.
    tombstoned: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    busy: BusyFn,
}

impl<E: DbExec + Clone> Backfill<E> {
    /// A backfill that never yields to live work. The app should use [`Backfill::gated`].
    pub fn new(db: E, tombstoned: Arc<dyn Fn(&str) -> bool + Send + Sync>) -> Arc<Self> {
        Self::gated(db, tombstoned, Arc::new(|| false))
    }

    /// A backfill that parks while `busy()` is true (N82).
    pub fn gated(db: E, tombstoned: Arc<dyn Fn(&str) -> bool + Send + Sync>, busy: BusyFn) -> Arc<Self> {
        Arc::new(Backfill {
            db,
            cancelled: Arc::new(AtomicBool::new(false)),
            cancel_tx: tokio::sync::watch::Sender::new(false),
            tombstoned,
            busy,
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.cancel_tx.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// One bounded pass over the meetings that miss vectors for `e`, newest meeting first.
    pub async fn pass(&self, e: &Arc<dyn Embedder>, max_lines: u32) -> Result<PassReport, LifecycleError> {
        self.pass_for(e, max_lines, None).await
    }

    /// [`Backfill::pass`] that also stops before the next batch once `superseded()` (ADR 0064: a reload replaced
    /// `e`).
    async fn pass_for(
        &self,
        e: &Arc<dyn Embedder>,
        max_lines: u32,
        superseded: Option<BusyFn>,
    ) -> Result<PassReport, LifecycleError> {
        let superseded = superseded.unwrap_or_else(|| Arc::new(|| false));
        // The batch gate inside a meeting: live work, or a newer model.
        let gate: BusyFn = {
            let (busy, sup) = (self.busy.clone(), superseded.clone());
            Arc::new(move || busy() || sup())
        };
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
            if superseded() {
                r.superseded = true;
                r.more = true;
                return Ok(r);
            }
            if (self.busy)() {
                r.paused = true;
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
            // `busy` is asked before each batch too, so a meeting that starts mid-pass waits for one batch at most;
            // that batch is still saved.
            r.embedded += index_vectors_paused(
                &self.db,
                e,
                max_lines - r.embedded,
                Some(id),
                Some(&stop),
                Some(&gate),
            )
            .await?;
            tokio::task::yield_now().await;
        }
        r.cancelled = self.is_cancelled();
        // A reload or the gate during the last meeting's batches: its remaining lines are still missing.
        r.superseded = !r.cancelled && superseded();
        r.paused = !r.cancelled && !r.superseded && (self.busy)();
        // At the bound the last meeting may have more lines left; the next pass finds out.
        r.more = r.cancelled || r.superseded || r.paused || r.embedded >= max_lines;
        Ok(r)
    }

    /// Waits for the model, then runs passes until nothing is missing or it is cancelled. A failed pass (busy DB,
    /// embedder error) is retried after `retry_after`; a paused one (the busy gate) after [`BUSY_POLL`]. Spawn it at
    /// boot. ADR 0064: each pass embeds for the current model, a reload stops a pass before its next batch, and with
    /// a reloadable embedder it waits for the next model when nothing is missing instead of ending.
    pub async fn run(self: Arc<Self>, embedder: Arc<LazyEmbedder>, retry_after: std::time::Duration) {
        let Some(mut e) = self.unless_cancelled(embedder.when_ready()).await else {
            return;
        };
        while !self.is_cancelled() {
            if let Some(now) = embedder.current() {
                e = now;
            }
            let superseded: BusyFn = {
                let (lazy, cur) = (embedder.clone(), e.clone());
                Arc::new(move || lazy.current().is_some_and(|n| !same_embedder(&n, &cur)))
            };
            match self.pass_for(&e, BACKFILL_PASS_LINES, Some(superseded)).await {
                Ok(r) if r.superseded => {}
                Ok(r) if !r.more => {
                    if !embedder.reloadable() {
                        return;
                    }
                    match self.unless_cancelled(embedder.replaced(&e)).await {
                        Some(next) => e = next,
                        None => return,
                    }
                }
                Ok(r) if r.paused => tokio::time::sleep(BUSY_POLL).await,
                Ok(_) => tokio::task::yield_now().await,
                Err(err) => {
                    tracing::warn!(code = %err.code, err = %err.message, "vector backfill pass failed");
                    tokio::time::sleep(retry_after).await;
                }
            }
        }
    }

    /// `f`, or `None` once [`Backfill::cancel`] is called.
    async fn unless_cancelled<T>(&self, f: impl Future<Output = T>) -> Option<T> {
        let mut rx = self.cancel_tx.subscribe();
        tokio::select! {
            v = f => Some(v),
            _ = rx.wait_for(|c| *c) => None,
        }
    }
}

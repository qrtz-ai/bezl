//! N86: one live-work counter the vector backfill yields to (`qrtz_search::Backfill::gated`, ADR 0055). It is
//! held from `starting` through `stopping` of a recording, while any pipeline step runs, and while a dictation is
//! live. Holders are RAII guards, so a panic or an early return can't leave it raised.
use async_trait::async_trait;
use qrtz_lifecycle::runtime::{FinalizeOutcome, IndexOutcome, StepExecutor, SummaryRequest};
use qrtz_model::{LifecycleError, MeetingSummary, PipelineContext};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default, Debug)]
pub struct LiveWork {
    n: AtomicUsize,
}

impl LiveWork {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn busy(&self) -> bool {
        self.n.load(Ordering::SeqCst) > 0
    }
    pub fn count(&self) -> usize {
        self.n.load(Ordering::SeqCst)
    }
    pub fn hold(self: &Arc<Self>) -> LiveGuard {
        self.n.fetch_add(1, Ordering::SeqCst);
        LiveGuard(self.clone())
    }
}

#[must_use]
pub struct LiveGuard(Arc<LiveWork>);
impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.n.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An edge-triggered hold: `set(true)` takes the guard once, `set(false)` releases it.
#[derive(Default)]
pub struct LiveLatch(Mutex<Option<LiveGuard>>);
impl LiveLatch {
    pub fn set(&self, live: &Arc<LiveWork>, on: bool) {
        let mut g = self.0.lock().unwrap_or_else(|p| p.into_inner());
        match (on, g.is_some()) {
            (true, false) => *g = Some(live.hold()),
            (false, true) => *g = None,
            _ => {}
        }
    }
}

/// Every step runs under a hold.
pub struct BusySteps {
    pub inner: Arc<dyn StepExecutor>,
    pub live: Arc<LiveWork>,
}

#[async_trait]
impl StepExecutor for BusySteps {
    async fn finalize(&self, id: String, ctx: PipelineContext) -> Result<FinalizeOutcome, LifecycleError> {
        let _g = self.live.hold();
        self.inner.finalize(id, ctx).await
    }
    async fn summary(
        &self,
        id: String,
        req: SummaryRequest,
    ) -> Result<Option<MeetingSummary>, LifecycleError> {
        let _g = self.live.hold();
        self.inner.summary(id, req).await
    }
    async fn index(&self, id: String) -> Result<IndexOutcome, LifecycleError> {
        let _g = self.live.hold();
        self.inner.index(id).await
    }
    async fn resummarize(&self, id: String, t: Option<String>) -> Result<MeetingSummary, LifecycleError> {
        let _g = self.live.hold();
        self.inner.resummarize(id, t).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guards_and_latches_balance() {
        let l = LiveWork::new();
        let latch = LiveLatch::default();
        latch.set(&l, true);
        latch.set(&l, true);
        assert_eq!(l.count(), 1);
        let g = l.hold();
        assert_eq!(l.count(), 2);
        drop(g);
        latch.set(&l, false);
        assert!(!l.busy());
    }
}

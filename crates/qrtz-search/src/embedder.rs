//! The embedder seam (TS `Embedder` in embeddings.ts). Search never waits for a model: [`LazyEmbedder::ready`]
//! answers at once, and until a model has loaded (or when none is configured) search and chat are full-text only.
//!
//! [`LazyEmbedder::reload`] (ADR 0064) loads again after the model files changed (MiniLM downloaded while the app
//! runs): the current state keeps serving until the new model is ready, a failed load (missing files, a hash
//! mismatch) leaves it as it was, and loads never overlap (a reload asked for during one runs once more after it).
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use qrtz_model::LifecycleError;
use tokio::sync::watch;

/// A sentence embedder. Vectors are L2-normalized, so a dot product is the cosine similarity.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Stored with every vector (`segment_vectors.model`): a different id re-embeds every line.
    fn id(&self) -> &str;
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError>;
}

/// `vecToBlob`: little-endian f32s (the `Float32Array` bytes on every platform qrtz ships).
pub fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// `blobToVec`.
pub fn blob_to_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// `dot` over the shorter length, accumulated in f64 like JS numbers.
pub fn dot(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).fold(0.0, |s, (x, y)| s + *x as f64 * *y as f64)
}

pub type LoadFuture = Pin<Box<dyn Future<Output = Result<Arc<dyn Embedder>, String>> + Send>>;
pub type Loader = Box<dyn Fn() -> LoadFuture + Send + Sync>;

#[derive(Clone)]
pub enum EmbedderState {
    /// Not asked for yet.
    Idle,
    Loading,
    Ready(Arc<dyn Embedder>),
    /// No embedder configured, or it failed to load: full-text only for this run (TS `embedderBroken`).
    Unavailable,
}

impl EmbedderState {
    fn settled(&self) -> bool {
        matches!(self, EmbedderState::Ready(_) | EmbedderState::Unavailable)
    }
}

/// Loads the model once, in the background, on first use (TS `getEmbedder` with `transformersEmbedder`), and again
/// on [`LazyEmbedder::reload`].
pub struct LazyEmbedder {
    loader: Option<Loader>,
    state: watch::Sender<EmbedderState>,
    started: Mutex<bool>,
    /// ADR 0064: one load at a time. `requested` counts asks; the running load serves every ask made before it began.
    loads: Mutex<Loads>,
    /// The newest ask a finished load served, and whether that load succeeded.
    done: watch::Sender<(u64, bool)>,
}

#[derive(Default)]
struct Loads {
    requested: u64,
    running: bool,
}

/// The same loaded model (not just the same id).
pub fn same_embedder(a: &Arc<dyn Embedder>, b: &Arc<dyn Embedder>) -> bool {
    std::ptr::eq(Arc::as_ptr(a) as *const (), Arc::as_ptr(b) as *const ())
}

impl LazyEmbedder {
    pub fn new(loader: Loader) -> Arc<Self> {
        Arc::new(LazyEmbedder {
            loader: Some(loader),
            state: watch::Sender::new(EmbedderState::Idle),
            started: Mutex::new(false),
            loads: Mutex::new(Loads::default()),
            done: watch::Sender::new((0, false)),
        })
    }

    /// The default: no model, full-text search only.
    pub fn none() -> Arc<Self> {
        Arc::new(LazyEmbedder {
            loader: None,
            state: watch::Sender::new(EmbedderState::Unavailable),
            started: Mutex::new(true),
            loads: Mutex::new(Loads::default()),
            done: watch::Sender::new((0, false)),
        })
    }

    /// An embedder that is already loaded (tests, or a model loaded elsewhere).
    pub fn ready_with(e: Arc<dyn Embedder>) -> Arc<Self> {
        Arc::new(LazyEmbedder {
            loader: None,
            state: watch::Sender::new(EmbedderState::Ready(e)),
            started: Mutex::new(true),
            loads: Mutex::new(Loads::default()),
            done: watch::Sender::new((0, false)),
        })
    }

    pub fn state(&self) -> EmbedderState {
        self.state.borrow().clone()
    }

    /// Starts the load (once). Needs a tokio runtime.
    pub fn start(self: &Arc<Self>) {
        let mut started = self.started.lock().unwrap();
        if *started {
            return;
        }
        *started = true;
        if self.loader.is_none() {
            return;
        }
        self.state.send_replace(EmbedderState::Loading);
        drop(started);
        self.ask();
    }

    /// Can [`LazyEmbedder::reload`] load anything (a loader is configured)?
    pub fn reloadable(&self) -> bool {
        self.loader.is_some()
    }

    /// ADR 0064: load the model again, e.g. after MiniLM was downloaded. The load starts at the call (needs a tokio
    /// runtime); await the result to learn whether it succeeded, or drop it. Until the new model is ready the
    /// current state keeps serving (full-text only if no model was loaded, else the old model); a failed load keeps
    /// it. A reload asked for while a load runs is served by one more load after it, so concurrent reloads never
    /// load in parallel and the last one always sees the files as they were when it was asked. `false` with no
    /// loader.
    pub fn reload(self: &Arc<Self>) -> impl Future<Output = bool> + Send + 'static {
        let ask = self.loader.as_ref().map(|_| {
            // A reload before first use is that first load.
            *self.started.lock().unwrap() = true;
            self.ask()
        });
        let me = self.clone();
        async move {
            let Some(ask) = ask else { return false };
            let mut rx = me.done.subscribe();
            let served = rx.wait_for(|(served, _)| *served >= ask).await.map(|d| d.1);
            served.unwrap_or(false)
        }
    }

    /// Records an ask and starts the load loop if none runs; the ask's number.
    fn ask(self: &Arc<Self>) -> u64 {
        let mut l = self.loads.lock().unwrap();
        l.requested += 1;
        let ask = l.requested;
        if !l.running {
            l.running = true;
            tokio::spawn(self.clone().drive());
        }
        ask
    }

    async fn drive(self: Arc<Self>) {
        let Some(loader) = &self.loader else { return };
        loop {
            let serves = self.loads.lock().unwrap().requested;
            // N102: the load runs as its own task, so a loader that panics fails this load (`Err`) instead of
            // killing the loop with `running` still set, which would leave every later reload waiting for good.
            let res = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(loader)) {
                Ok(fut) => match tokio::spawn(fut).await {
                    Ok(r) => r,
                    Err(e) => Err(format!("embedder load panicked: {e}")),
                },
                Err(_) => Err("embedder load panicked".to_string()),
            };
            let ok = res.is_ok();
            self.state.send_modify(|s| match res {
                Ok(e) => {
                    if matches!(s, EmbedderState::Ready(_)) {
                        tracing::info!(model = e.id(), "embedder reloaded");
                    }
                    *s = EmbedderState::Ready(e);
                }
                // The old model, if any, keeps serving; otherwise full-text only, as before.
                Err(err) => {
                    tracing::warn!(%err, "embeddings unavailable; search and chat use full-text retrieval");
                    if !matches!(s, EmbedderState::Ready(_)) {
                        *s = EmbedderState::Unavailable;
                    }
                }
            });
            self.done.send_replace((serves, ok));
            let mut l = self.loads.lock().unwrap();
            if l.requested == serves {
                l.running = false;
                return;
            }
        }
    }

    /// The loaded model, without starting a load.
    pub fn current(&self) -> Option<Arc<dyn Embedder>> {
        match &*self.state.borrow() {
            EmbedderState::Ready(e) => Some(e.clone()),
            _ => None,
        }
    }

    /// Never waits: the embedder if it has loaded, else `None` (and the load is started).
    pub fn ready(self: &Arc<Self>) -> Option<Arc<dyn Embedder>> {
        self.start();
        match &*self.state.borrow() {
            EmbedderState::Ready(e) => Some(e.clone()),
            _ => None,
        }
    }

    /// Waits for the load to settle (the index step, under its deadline). `None`: no embedder.
    pub async fn get(self: &Arc<Self>) -> Option<Arc<dyn Embedder>> {
        self.start();
        let mut rx = self.state.subscribe();
        let s = rx.wait_for(EmbedderState::settled).await.ok()?.clone();
        match s {
            EmbedderState::Ready(e) => Some(e),
            _ => None,
        }
    }

    /// Resolves when a model is ready (for starting the backfill): at once if one is loaded, else when a load or a
    /// [`LazyEmbedder::reload`] succeeds; never without one.
    pub async fn when_ready(self: &Arc<Self>) -> Arc<dyn Embedder> {
        self.start();
        self.ready_where(|_| true).await
    }

    /// Resolves when a model other than `than` is ready (a reload replaced it).
    pub async fn replaced(&self, than: &Arc<dyn Embedder>) -> Arc<dyn Embedder> {
        let than = than.clone();
        self.ready_where(move |e| !same_embedder(e, &than)).await
    }

    async fn ready_where(&self, f: impl Fn(&Arc<dyn Embedder>) -> bool) -> Arc<dyn Embedder> {
        let mut rx = self.state.subscribe();
        let got = rx
            .wait_for(|s| matches!(s, EmbedderState::Ready(e) if f(e)))
            .await
            .ok()
            .and_then(|s| match &*s {
                EmbedderState::Ready(e) => Some(e.clone()),
                _ => None,
            });
        match got {
            Some(e) => e,
            None => std::future::pending().await,
        }
    }
}

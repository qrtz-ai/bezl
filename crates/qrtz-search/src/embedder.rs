//! The embedder seam (TS `Embedder` in embeddings.ts). Search never waits for a model: [`LazyEmbedder::ready`]
//! answers at once, and until a model has loaded (or when none is configured) search and chat are full-text only.
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

/// Loads the model once, in the background, on first use (TS `getEmbedder` with `transformersEmbedder`).
pub struct LazyEmbedder {
    loader: Option<Loader>,
    state: watch::Sender<EmbedderState>,
    started: Mutex<bool>,
}

impl LazyEmbedder {
    pub fn new(loader: Loader) -> Arc<Self> {
        Arc::new(LazyEmbedder {
            loader: Some(loader),
            state: watch::Sender::new(EmbedderState::Idle),
            started: Mutex::new(false),
        })
    }

    /// The default: no model, full-text search only.
    pub fn none() -> Arc<Self> {
        Arc::new(LazyEmbedder {
            loader: None,
            state: watch::Sender::new(EmbedderState::Unavailable),
            started: Mutex::new(true),
        })
    }

    /// An embedder that is already loaded (tests, or a model loaded elsewhere).
    pub fn ready_with(e: Arc<dyn Embedder>) -> Arc<Self> {
        Arc::new(LazyEmbedder {
            loader: None,
            state: watch::Sender::new(EmbedderState::Ready(e)),
            started: Mutex::new(true),
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
        let Some(loader) = &self.loader else { return };
        self.state.send_replace(EmbedderState::Loading);
        let fut = loader();
        let me = self.clone();
        tokio::spawn(async move {
            let s = match fut.await {
                Ok(e) => EmbedderState::Ready(e),
                Err(err) => {
                    tracing::warn!(%err, "embeddings unavailable; search and chat use full-text retrieval");
                    EmbedderState::Unavailable
                }
            };
            me.state.send_replace(s);
        });
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

    /// Resolves when the embedder becomes ready (for starting the backfill); never if it is unavailable.
    pub async fn when_ready(self: &Arc<Self>) -> Arc<dyn Embedder> {
        match self.get().await {
            Some(e) => e,
            None => std::future::pending().await,
        }
    }
}

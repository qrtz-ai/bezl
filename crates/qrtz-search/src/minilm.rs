//! The local all-MiniLM-L6-v2 embedder through candle (ADR 0050): the same model as the Electron worker
//! (`Xenova/all-MiniLM-L6-v2`, transformers.js), mean pooling over the attention mask, L2-normalized, 384-d. CPU
//! only, on a blocking thread. Weights are not bundled: [`MiniLm::load`] reads a model directory (`config.json`,
//! `tokenizer.json`, `model.safetensors`), checking each file against the pinned [`FILES`] hashes first, and
//! `minilm-download` can fetch that revision. Meeting text never leaves the Mac.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use qrtz_model::LifecycleError;
use sha2::{Digest, Sha256};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

use crate::embedder::Embedder;

/// Hugging Face repo and pinned revision of the weights.
pub const REPO: &str = "sentence-transformers/all-MiniLM-L6-v2";
pub const REVISION: &str = "1110a243fdf4706b3f48f1d95db1a4f5529b4d41";
/// Stored with every vector. fp32 candle vectors differ from the Electron q8 ones (`Xenova/all-MiniLM-L6-v2@q8`),
/// so the id differs too and the backfill re-embeds once.
pub const MODEL_ID: &str = "all-MiniLM-L6-v2@candle-f32";
/// transformers.js truncates at the tokenizer's `model_max_length` (512), not at the 128 in this repo's
/// tokenizer.json.
pub const MAX_TOKENS: usize = 512;
/// The model files with their SHA-256 at [`REVISION`].
pub const FILES: [(&str, &str); 3] = [
    (
        "config.json",
        "953f9c0d463486b10a6871cc2fd59f223b2c70184f49815e7efbcab5d8908b41",
    ),
    (
        "tokenizer.json",
        "be50c3628f2bf5bb5e3a7f17b1f74611b2561a3a27eeab05e5aa30f411572037",
    ),
    (
        "model.safetensors",
        "53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db",
    ),
];

fn err(code: &str, m: impl std::fmt::Display, retryable: bool) -> LifecycleError {
    LifecycleError {
        code: code.into(),
        message: m.to_string(),
        retryable,
    }
}

struct Inner {
    model: BertModel,
    tokenizer: Tokenizer,
}

pub struct MiniLm {
    id: String,
    inner: Arc<Mutex<Inner>>,
}

/// Checks a model directory against [`FILES`]. `Err` names the first file that is missing or differs.
pub fn verify_dir(dir: &Path) -> Result<(), String> {
    for (name, want) in FILES {
        let bytes = std::fs::read(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
        let got = format!("{:x}", Sha256::digest(&bytes));
        if got != want {
            return Err(format!("{name}: sha256 {got} is not the pinned {want}"));
        }
    }
    Ok(())
}

impl MiniLm {
    /// Loads a verified model directory (blocking: run it off the async workers).
    pub fn load(dir: &Path) -> Result<MiniLm, String> {
        verify_dir(dir)?;
        Self::load_unverified(dir, MODEL_ID)
    }

    /// Any BERT directory with these three files, under `id` (tests use a tiny random model).
    pub fn load_unverified(dir: &Path, id: &str) -> Result<MiniLm, String> {
        let s = |e: &dyn std::fmt::Display| e.to_string();
        let config: Config =
            serde_json::from_slice(&std::fs::read(dir.join("config.json")).map_err(|e| s(&e))?)
                .map_err(|e| s(&e))?;
        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| s(&e))?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_TOKENS,
                ..Default::default()
            }))
            .map_err(|e| s(&e))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        }));
        let weights = std::fs::read(dir.join("model.safetensors")).map_err(|e| s(&e))?;
        let vb =
            VarBuilder::from_buffered_safetensors(weights, DType::F32, &Device::Cpu).map_err(|e| s(&e))?;
        let model = BertModel::load(vb, &config).map_err(|e| s(&e))?;
        Ok(MiniLm {
            id: id.to_string(),
            inner: Arc::new(Mutex::new(Inner { model, tokenizer })),
        })
    }

    /// The vectors for `texts`, synchronously.
    pub fn embed_sync(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError> {
        embed_blocking(&self.inner.lock().unwrap(), texts)
    }
}

fn embed_blocking(m: &Inner, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError> {
    if texts.is_empty() {
        return Ok(vec![]);
    }
    let e = |x: &dyn std::fmt::Display| err("E_INTERNAL", format!("embed: {x}"), false);
    let enc = m
        .tokenizer
        .encode_batch(texts.to_vec(), true)
        .map_err(|x| e(&x))?;
    let n = enc.len();
    let len = enc[0].get_ids().len();
    let flat = |f: &dyn Fn(&tokenizers::Encoding) -> &[u32]| -> Vec<u32> {
        enc.iter().flat_map(|x| f(x).to_vec()).collect()
    };
    let dev = Device::Cpu;
    let ids = Tensor::from_vec(flat(&|x| x.get_ids()), (n, len), &dev).map_err(|x| e(&x))?;
    let types = Tensor::from_vec(flat(&|x| x.get_type_ids()), (n, len), &dev).map_err(|x| e(&x))?;
    let mask = Tensor::from_vec(flat(&|x| x.get_attention_mask()), (n, len), &dev).map_err(|x| e(&x))?;
    let out = (|| -> candle_core::Result<Vec<Vec<f32>>> {
        let h = m.model.forward(&ids, &types, Some(&mask))?; // (n, len, 384)
        let maskf = mask.to_dtype(DType::F32)?.unsqueeze(2)?; // (n, len, 1)
        let sum = h.broadcast_mul(&maskf)?.sum(1)?; // (n, 384)
        let count = maskf.sum(1)?.clamp(1e-9f32, f32::MAX)?; // (n, 1)
        let mean = sum.broadcast_div(&count)?;
        let norm = mean.sqr()?.sum_keepdim(1)?.sqrt()?.clamp(1e-12f32, f32::MAX)?;
        mean.broadcast_div(&norm)?.to_vec2::<f32>()
    })()
    .map_err(|x| e(&x))?;
    Ok(out)
}

#[async_trait]
impl Embedder for MiniLm {
    fn id(&self) -> &str {
        &self.id
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError> {
        let (inner, texts) = (self.inner.clone(), texts.to_vec());
        tokio::task::spawn_blocking(move || embed_blocking(&inner.lock().unwrap(), &texts))
            .await
            .map_err(|x| err("E_INTERNAL", format!("embed task: {x}"), false))?
    }
}

/// A [`crate::LazyEmbedder`] loader for a model directory: loads on a blocking thread, `Err` (full-text only) when
/// the files are missing or don't match the pinned hashes.
pub fn loader(dir: PathBuf) -> crate::embedder::Loader {
    Box::new(move || {
        let dir = dir.clone();
        Box::pin(async move {
            let m = tokio::task::spawn_blocking(move || MiniLm::load(&dir))
                .await
                .map_err(|e| e.to_string())??;
            Ok(Arc::new(m) as Arc<dyn Embedder>)
        })
    })
}

/// Where [`download`] fetches from in the app.
pub const HF_BASE: &str = "https://huggingface.co";

/// Fetches the pinned revision from `base` (normally [`HF_BASE`]) into `dir`: each file is hash-checked, written to
/// a `.part` file, then renamed, so a bad or partial download never sits under the real name. Files already there
/// with the right hash are kept.
#[cfg(feature = "minilm-download")]
pub async fn download(client: &reqwest::Client, base: &str, dir: &Path) -> Result<(), String> {
    tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
    for (name, want) in FILES {
        let path = dir.join(name);
        if let Ok(b) = tokio::fs::read(&path).await {
            if format!("{:x}", Sha256::digest(&b)) == want {
                continue;
            }
        }
        let url = format!("{base}/{REPO}/resolve/{REVISION}/{name}");
        let res = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("{name}: {e}"))?;
        if !res.status().is_success() {
            return Err(format!("{name}: HTTP {}", res.status()));
        }
        let bytes = res.bytes().await.map_err(|e| format!("{name}: {e}"))?;
        let got = format!("{:x}", Sha256::digest(&bytes));
        if got != want {
            return Err(format!("{name}: sha256 {got} is not the pinned {want}"));
        }
        let part = dir.join(format!("{name}.part"));
        tokio::fs::write(&part, &bytes).await.map_err(|e| e.to_string())?;
        tokio::fs::rename(&part, &path).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

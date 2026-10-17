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
    verify_files(dir, &FILES)
}

/// [`verify_dir`] for any pinned file list (tests use small files with their own hashes). N105: each file is hashed
/// as it streams (64 KiB at a time), never read whole, and a file already verified this run whose identity has not
/// changed since (device, inode, size, mtime, ctime) is not hashed again, so the boot path (status, loader, load)
/// hashes the 90 MB weights once instead of three times. Any write, replace or rename gives the file a new identity.
pub fn verify_files(dir: &Path, files: &[(&str, &str)]) -> Result<(), String> {
    for (name, want) in files {
        verify_file(&dir.join(name), want).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(())
}

/// What identifies one version of a file on disk (unix: changes on every write, rename over or replace).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FileId {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

#[cfg(unix)]
fn file_id(m: &std::fs::Metadata) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    Some(FileId {
        dev: m.dev(),
        ino: m.ino(),
        len: m.len(),
        mtime: (m.mtime(), m.mtime_nsec()),
        ctime: (m.ctime(), m.ctime_nsec()),
    })
}

#[cfg(not(unix))]
fn file_id(_m: &std::fs::Metadata) -> Option<FileId> {
    None // no stable identity: always hash
}

/// Files that matched their pinned hash this run: path → (identity, hash).
fn verified() -> &'static Mutex<std::collections::HashMap<PathBuf, (FileId, String)>> {
    static V: std::sync::OnceLock<Mutex<std::collections::HashMap<PathBuf, (FileId, String)>>> =
        std::sync::OnceLock::new();
    V.get_or_init(Default::default)
}

fn known_good(path: &Path, id: Option<FileId>, want: &str) -> bool {
    id.is_some_and(|id| {
        verified()
            .lock()
            .unwrap()
            .get(path)
            .is_some_and(|(i, h)| *i == id && h == want)
    })
}

/// Remembers `path` as matching `want` if the open file `f` still has identity `id` (nothing wrote it meanwhile).
fn remember(path: &Path, f: &std::fs::File, id: Option<FileId>, want: &str) {
    let after = f.metadata().ok().as_ref().and_then(file_id);
    if let (Some(id), Some(after)) = (id, after) {
        if id == after {
            verified()
                .lock()
                .unwrap()
                .insert(path.to_path_buf(), (id, want.to_string()));
        }
    }
}

fn verify_file(path: &Path, want: &str) -> Result<(), String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let id = f.metadata().ok().as_ref().and_then(file_id);
    if known_good(path, id, want) {
        return Ok(());
    }
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    let got = format!("{:x}", hash.finalize());
    if got != want {
        return Err(format!("sha256 {got} is not the pinned {want}"));
    }
    remember(path, &f, id, want);
    Ok(())
}

/// Reads a pinned file once and checks the bytes it returns (N105: what is loaded is what was checked, no second
/// read between a check and the load).
fn read_pinned(path: &Path, want: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let id = f.metadata().ok().as_ref().and_then(file_id);
    let mut bytes = Vec::with_capacity(id.map_or(0, |i| i.len as usize));
    f.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if !known_good(path, id, want) {
        let got = format!("{:x}", Sha256::digest(&bytes));
        if got != want {
            return Err(format!("sha256 {got} is not the pinned {want}"));
        }
        remember(path, &f, id, want);
    }
    Ok(bytes)
}

impl MiniLm {
    /// Loads a verified model directory (blocking: run it off the async workers).
    pub fn load(dir: &Path) -> Result<MiniLm, String> {
        Self::load_pinned(dir, &FILES, MODEL_ID)
    }

    /// Loads `config.json`, `tokenizer.json` and `model.safetensors` from `dir`, each read once and checked against
    /// its hash in `files` (tests pin a tiny model this way).
    pub fn load_pinned(dir: &Path, files: &[(&str, &str)], id: &str) -> Result<MiniLm, String> {
        let pinned = |name: &str| -> Result<Vec<u8>, String> {
            let want = files
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, h)| *h)
                .ok_or_else(|| format!("{name}: not pinned"))?;
            read_pinned(&dir.join(name), want).map_err(|e| format!("{name}: {e}"))
        };
        Self::from_bytes(
            &pinned("config.json")?,
            &pinned("tokenizer.json")?,
            pinned("model.safetensors")?,
            id,
        )
    }

    /// Any BERT directory with these three files, under `id` (tests use a tiny random model).
    pub fn load_unverified(dir: &Path, id: &str) -> Result<MiniLm, String> {
        let read = |n: &str| std::fs::read(dir.join(n)).map_err(|e| format!("{n}: {e}"));
        Self::from_bytes(
            &read("config.json")?,
            &read("tokenizer.json")?,
            read("model.safetensors")?,
            id,
        )
    }

    fn from_bytes(config: &[u8], tokenizer: &[u8], weights: Vec<u8>, id: &str) -> Result<MiniLm, String> {
        let s = |e: &dyn std::fmt::Display| e.to_string();
        let config: Config = serde_json::from_slice(config).map_err(|e| s(&e))?;
        let mut tokenizer = Tokenizer::from_bytes(tokenizer).map_err(|e| s(&e))?;
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

/// Bounds on one [`download`] (N99). The defaults fit the pinned files (`model.safetensors` is about 90 MB).
#[cfg(feature = "minilm-download")]
#[derive(Clone, Copy, Debug)]
pub struct DownloadLimits {
    /// A file larger than this is refused (from `Content-Length`, or once the body passes it), never buffered.
    pub max_file_bytes: u64,
    /// No body bytes for this long (a dead connection after a network loss, a stalled CDN) fails the download.
    pub idle: std::time::Duration,
}

#[cfg(feature = "minilm-download")]
impl Default for DownloadLimits {
    fn default() -> Self {
        DownloadLimits {
            max_file_bytes: 256 * 1024 * 1024,
            idle: std::time::Duration::from_secs(60),
        }
    }
}

/// Fetches the pinned revision from `base` (normally [`HF_BASE`]) into `dir`: each file streams into a `.part`
/// file while it is hashed, is synced, and only then renamed, so a bad or partial download never sits under the real
/// name. Files already there with the right hash are kept. N99: the body is never held in memory, a file over
/// [`DownloadLimits::max_file_bytes`] is refused, and a body that stalls for [`DownloadLimits::idle`] fails instead
/// of hanging the search-model status in `downloading` for good. A failed file leaves no `.part` behind.
#[cfg(feature = "minilm-download")]
pub async fn download(client: &reqwest::Client, base: &str, dir: &Path) -> Result<(), String> {
    download_files(client, base, dir, &FILES, DownloadLimits::default()).await
}

/// [`download`] for any pinned file list (tests use small files with their own hashes).
#[cfg(feature = "minilm-download")]
pub async fn download_files(
    client: &reqwest::Client,
    base: &str,
    dir: &Path,
    files: &[(&str, &str)],
    limits: DownloadLimits,
) -> Result<(), String> {
    tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
    for (name, want) in files {
        let path = dir.join(name);
        // N105: streamed, not read whole.
        let (p, w) = (path.clone(), want.to_string());
        if tokio::task::spawn_blocking(move || verify_file(&p, &w))
            .await
            .is_ok_and(|r| r.is_ok())
        {
            continue;
        }
        let url = format!("{base}/{REPO}/resolve/{REVISION}/{name}");
        let part = dir.join(format!("{name}.part"));
        let r = fetch_one(client, &url, &part, want, limits).await;
        let r = match r {
            Ok(()) => tokio::fs::rename(&part, &path).await.map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        if let Err(e) = r {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(format!("{name}: {e}"));
        }
    }
    Ok(())
}

#[cfg(feature = "minilm-download")]
async fn fetch_one(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    want: &str,
    limits: DownloadLimits,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let idle = |what: &str| format!("{what}: no data for {} s", limits.idle.as_secs());
    let mut res = tokio::time::timeout(limits.idle, client.get(url).send())
        .await
        .map_err(|_| idle("request"))?
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("HTTP {}", res.status()));
    }
    if res.content_length().is_some_and(|n| n > limits.max_file_bytes) {
        return Err(format!("larger than {} bytes", limits.max_file_bytes));
    }
    let mut out = tokio::fs::File::create(part).await.map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut got: u64 = 0;
    loop {
        let chunk = tokio::time::timeout(limits.idle, res.chunk())
            .await
            .map_err(|_| idle("body"))?
            .map_err(|e| e.to_string())?;
        let Some(chunk) = chunk else { break };
        got += chunk.len() as u64;
        if got > limits.max_file_bytes {
            return Err(format!("larger than {} bytes", limits.max_file_bytes));
        }
        hash.update(&chunk);
        out.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    let digest = format!("{:x}", hash.finalize());
    if digest != want {
        return Err(format!("sha256 {digest} is not the pinned {want}"));
    }
    out.sync_all().await.map_err(|e| e.to_string())?;
    Ok(())
}

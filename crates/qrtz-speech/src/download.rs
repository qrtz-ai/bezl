//! Resumable, size-limited bundle download (ADR 0046 §4). HTTPS is transport only: nothing fetched is trusted
//! before the manifest signature and the per-file sha256 check.
//!
//! Server layout under `base`: `latest/manifest.json` + `.sig`, files at `<version>/<path>`, optional
//! `denylist.json` + `denylist.json.sig` (required for installs, N60) and `key-rotation.json` +
//! `key-rotation.sigs.json`.
use crate::bundle::{Manifest, MAX_META_BYTES};
use crate::fsx;
use crate::SpeechError;
use futures_util::StreamExt;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// Pause = stop the transfer and keep the `.part` files; calling `download_bundle` again resumes.
#[derive(Clone, Default)]
pub struct DownloadControl {
    paused: Arc<AtomicBool>,
}

impl DownloadControl {
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

pub struct Downloader {
    client: reqwest::Client,
    base: String,
}

impl Downloader {
    pub fn new(base: &str) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(std::time::Duration::from_secs(60))
            .https_only(!base.starts_with("http://127.0.0.1") && !base.starts_with("http://localhost"))
            .build()
            .expect("http client");
        Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    /// A small signed document; `Ok(None)` on 404.
    pub async fn fetch_meta(&self, path: &str) -> Result<Option<Vec<u8>>, SpeechError> {
        let r = self
            .client
            .get(format!("{}/{path}", self.base))
            .send()
            .await
            .map_err(|e| SpeechError::Network(e.to_string()))?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !r.status().is_success() {
            return Err(SpeechError::Network(format!("{path}: http {}", r.status())));
        }
        if r.content_length().is_some_and(|n| n > MAX_META_BYTES as u64) {
            return Err(SpeechError::TooLarge);
        }
        let mut out = Vec::new();
        let mut s = r.bytes_stream();
        while let Some(c) = s.next().await {
            let c = c.map_err(|e| SpeechError::Network(e.to_string()))?;
            if out.len() + c.len() > MAX_META_BYTES {
                return Err(SpeechError::TooLarge);
            }
            out.extend_from_slice(&c);
        }
        Ok(Some(out))
    }

    /// Download every file of `m` into `staging` (resuming `.part` files, skipping finished ones). Each file is
    /// capped at its manifest size and checked against its sha256 before it is renamed into place.
    pub async fn download_bundle(
        &self,
        m: &Manifest,
        staging: &Path,
        ctl: &DownloadControl,
        on_progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<(), SpeechError> {
        let total = m.total_size();
        let mut done_files = 0u64;
        for f in &m.files {
            let dest = staging.join(&f.path);
            if let Some(parent) = dest.parent() {
                fsx::mkdir_private(parent)?;
            }
            if dest.exists() {
                let (n, h) = fsx::sha256_file(&dest)?;
                if n == f.size && h == f.sha256 {
                    done_files += f.size;
                    on_progress(Progress {
                        done: done_files,
                        total,
                    });
                    continue;
                }
                std::fs::remove_file(&dest)?;
            }
            let part = staging.join(format!("{}.part", f.path));
            let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if have > f.size {
                std::fs::remove_file(&part)?;
                have = 0;
            }
            if have < f.size {
                if ctl.is_paused() {
                    return Err(SpeechError::Paused);
                }
                let mut req = self.client.get(format!("{}/{}/{}", self.base, m.version, f.path));
                if have > 0 {
                    req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
                }
                let r = req
                    .send()
                    .await
                    .map_err(|e| SpeechError::Network(e.to_string()))?;
                let status = r.status();
                let mut file = if have > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
                    fsx::open_private_append(&part)?
                } else if status.is_success() {
                    have = 0;
                    let _ = std::fs::remove_file(&part);
                    fsx::open_private_append(&part)?
                } else {
                    return Err(SpeechError::Network(format!("{}: http {status}", f.path)));
                };
                let mut s = r.bytes_stream();
                let mut last_report = 0u64;
                while let Some(c) = s.next().await {
                    if ctl.is_paused() {
                        file.flush()?;
                        return Err(SpeechError::Paused);
                    }
                    let c = c.map_err(|e| SpeechError::Network(e.to_string()))?;
                    if have + c.len() as u64 > f.size {
                        drop(file);
                        let _ = std::fs::remove_file(&part);
                        return Err(SpeechError::TooLarge);
                    }
                    file.write_all(&c)?;
                    have += c.len() as u64;
                    if have - last_report >= 256 * 1024 || have == f.size {
                        last_report = have;
                        on_progress(Progress {
                            done: done_files + have,
                            total,
                        });
                    }
                }
                file.sync_all()?;
                if have < f.size {
                    return Err(SpeechError::Network(format!(
                        "{}: connection closed early",
                        f.path
                    )));
                }
            }
            let (n, h) = fsx::sha256_file(&part)?;
            if n != f.size || h != f.sha256 {
                let _ = std::fs::remove_file(&part);
                return Err(SpeechError::Checksum(f.path.clone()));
            }
            std::fs::rename(&part, &dest)?;
            done_files += f.size;
            on_progress(Progress {
                done: done_files,
                total,
            });
        }
        Ok(())
    }
}

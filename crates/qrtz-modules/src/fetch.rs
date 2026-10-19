//! The one fetcher (ADR 0157 §6): every blob, of every package kind, from every source, through here.
//!
//! - **Sources**, in order: each module host (`modules.qrtz.ai`, then its mirrors) at `v1/blobs/sha256/<hex>`, then
//!   the blob's own extra mirror URLs from the signed tree (a model's pinned Hugging Face URL). A source that fails,
//!   or whose bytes don't hash right, is dropped for the next one.
//! - **Parallel ranges**: a blob is fetched as [`RANGE_BYTES`] ranges, [`PARALLEL_RANGES`] at a time, written in
//!   place into a preallocated `store/.tmp/<sha256>.part`; [`PARALLEL_BLOBS`] blobs go at once.
//! - **Dead sources cost one round** (N176, ADR 0159): once a range from a source fails, that blob starts no more
//!   ranges from it; once a module host has failed [`DEAD_HOST_AFTER`] blobs and served none, the rest of this fetch
//!   skips it. An unpublished or unreachable host (a firewall that drops packets, a captive portal) then delays the
//!   first install by about one timeout per host instead of one per range of every blob.
//! - **Resumable**: finished range numbers are appended to `<sha256>.done`; a restart fetches only the rest.
//! - **Verified**: a blob enters the store only after its whole size and sha256 match ([`Store::insert_file`]);
//!   a mismatch discards the part and its ranges.
//!
//! The fetcher has no network code: bytes come through [`Transport::get_range`] (qrtz-app's HTTPS transport over
//! qrtz-speech's downloader, or a `.qrtzpkg`).
use crate::index::BlobRef;
use crate::store::Store;
use futures_util::future::{BoxFuture, FutureExt};
use futures_util::stream::{self, StreamExt};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// In a fetch error, a source whose bytes didn't hash to the signed sha256 (callers report it as a checksum error).
pub const MISMATCH: &str = "bytes didn't match";
pub const RANGE_BYTES: u64 = 8 * 1024 * 1024;
pub const PARALLEL_RANGES: usize = 4;
pub const PARALLEL_BLOBS: usize = 3;
/// A module host that has failed this many blobs in one fetch, and served none, is skipped for the rest of it.
pub const DEAD_HOST_AFTER: u64 = 2;

/// Where one blob's bytes can come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Module host `n` (0 = primary), at `v1/blobs/sha256/<hex>`.
    Host(usize),
    /// An extra mirror URL from the signed tree.
    Url(String),
}

/// A source of layout paths and byte ranges: the HTTPS hosts or a `.qrtzpkg`.
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    fn hosts(&self) -> usize;
    /// A small document; `Ok(None)` when the host doesn't have it.
    async fn get(&self, host: usize, path: &str, max: usize) -> Result<Option<Vec<u8>>, String>;
    /// Exactly `len` bytes of blob `sha256` starting at `start`, from `src`. Anything else is an error.
    async fn get_range(&self, src: &Source, sha256: &str, start: u64, len: u64) -> Result<Vec<u8>, String>;
}

fn range_count(size: u64) -> u64 {
    size.div_ceil(RANGE_BYTES)
}

fn done_set(p: &Path) -> BTreeSet<u64> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

#[cfg(unix)]
fn write_at(p: &Path, off: u64, b: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    let f = std::fs::OpenOptions::new().write(true).open(p)?;
    f.write_all_at(b, off)?;
    f.sync_data()
}

#[cfg(not(unix))]
fn write_at(p: &Path, off: u64, b: &[u8]) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new().write(true).open(p)?;
    f.seek(SeekFrom::Start(off))?;
    f.write_all(b)?;
    f.sync_data()
}

/// Fetch every blob missing from the store. `progress(done, total)` counts bytes, blobs already stored included.
pub async fn fetch_blobs(
    t: &dyn Transport,
    store: &Store,
    blobs: &[BlobRef],
    progress: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<(), String> {
    let total: u64 = blobs.iter().map(|b| b.size).sum();
    let done = AtomicU64::new(0);
    crate::fsutil::mkdir_private(&store.tmp()).map_err(|e| e.to_string())?;
    // Boxed (`Send`) futures built up front: a closure in `StreamExt::map` makes the caller's future non-`Send`
    // under the async-trait lifetimes (rust-lang/rust#102211).
    let done = &done;
    // Per module host: blobs it failed, blobs it served (N176).
    let failed: Vec<AtomicU64> = (0..t.hosts()).map(|_| AtomicU64::new(0)).collect();
    let served: Vec<AtomicU64> = (0..t.hosts()).map(|_| AtomicU64::new(0)).collect();
    let (failed, served) = (&failed, &served);
    let futs: Vec<BoxFuture<'_, Result<(), String>>> = blobs
        .iter()
        .map(|b| {
            async move {
                if store.has(&b.sha256, b.size)
                    && store.verify(&b.sha256, b.size).map_err(|e| e.to_string())?
                {
                    progress(done.fetch_add(b.size, Ordering::SeqCst) + b.size, total);
                    return Ok(());
                }
                let mut errs = vec![];
                let sources = (0..t.hosts())
                    .map(Source::Host)
                    .chain(b.mirrors.iter().cloned().map(Source::Url));
                for src in sources {
                    let host = match src {
                        Source::Host(i) => Some(i),
                        Source::Url(_) => None,
                    };
                    if let Some(i) = host {
                        let f = failed[i].load(Ordering::SeqCst);
                        if f >= DEAD_HOST_AFTER && served[i].load(Ordering::SeqCst) == 0 {
                            errs.push(format!("{src:?}: skipped, failed {f} blobs in this fetch"));
                            continue;
                        }
                    }
                    let r = fetch_one(t, store, b, &src, done, total, progress).await;
                    if let Some(i) = host {
                        let n = if matches!(r, Ok(true)) { served } else { failed };
                        n[i].fetch_add(1, Ordering::SeqCst);
                    }
                    match r {
                        Ok(true) => return Ok(()),
                        Ok(false) => errs.push(format!("{src:?}: {MISMATCH}")),
                        Err(e) => errs.push(format!("{src:?}: {e}")),
                    }
                }
                Err(format!("{}: every source failed ({})", b.sha256, errs.join("; ")))
            }
            .boxed()
        })
        .collect();
    let results: Vec<Result<(), String>> = stream::iter(futs)
        .buffer_unordered(PARALLEL_BLOBS)
        .collect()
        .await;
    let errs: Vec<String> = results.into_iter().filter_map(Result::err).collect();
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

async fn fetch_one(
    t: &dyn Transport,
    store: &Store,
    b: &BlobRef,
    src: &Source,
    done: &AtomicU64,
    total: u64,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<bool, String> {
    let io = |e: std::io::Error| e.to_string();
    let part = store.tmp().join(format!("{}.part", b.sha256));
    let marks = store.tmp().join(format!("{}.done", b.sha256));
    if std::fs::metadata(&part).map(|m| m.len()).ok() != Some(b.size) {
        let _ = std::fs::remove_file(&marks);
        let mut o = std::fs::OpenOptions::new();
        o.create(true).write(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        o.open(&part).map_err(io)?.set_len(b.size).map_err(io)?;
    }
    let have = done_set(&marks);
    let n = range_count(b.size);
    let already: u64 = have
        .iter()
        .filter(|i| **i < n)
        .map(|i| (b.size - i * RANGE_BYTES).min(RANGE_BYTES))
        .sum();
    progress(done.fetch_add(already, Ordering::SeqCst) + already, total);
    let log = Mutex::new(());
    let pending: Vec<u64> = (0..n).filter(|i| !have.contains(i)).collect();
    // Once a range from this source fails, ranges after the first round don't ask it (N176). The first round
    // always runs, so a one-off failed range still leaves its neighbours on disk for the resume.
    let gave_up = AtomicBool::new(false);
    let gave_up = &gave_up;
    let (part_r, marks_r, log_r) = (&part, &marks, &log);
    let futs: Vec<BoxFuture<'_, Result<(), String>>> = pending
        .into_iter()
        .enumerate()
        .map(|(k, i)| {
            let (part, marks, log) = (part_r, marks_r, log_r);
            async move {
                let start = i * RANGE_BYTES;
                let len = (b.size - start).min(RANGE_BYTES);
                if k >= PARALLEL_RANGES && gave_up.load(Ordering::SeqCst) {
                    return Err("an earlier range from this source failed".into());
                }
                let bytes = match t.get_range(src, &b.sha256, start, len).await {
                    Ok(b) => b,
                    Err(e) => {
                        gave_up.store(true, Ordering::SeqCst);
                        return Err(e);
                    }
                };
                if bytes.len() as u64 != len {
                    gave_up.store(true, Ordering::SeqCst);
                    return Err(format!("range {i}: {} bytes, wanted {len}", bytes.len()));
                }
                write_at(part, start, &bytes).map_err(|e| e.to_string())?;
                {
                    let _g = log.lock().expect("log");
                    use std::io::Write;
                    let mut f = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(marks)
                        .map_err(|e| e.to_string())?;
                    writeln!(f, "{i}").map_err(|e| e.to_string())?;
                }
                progress(done.fetch_add(len, Ordering::SeqCst) + len, total);
                Ok(())
            }
            .boxed()
        })
        .collect();
    let results: Vec<Result<(), String>> = stream::iter(futs)
        .buffer_unordered(PARALLEL_RANGES)
        .collect()
        .await;
    // The first real error, not a range that only stopped because of it.
    let mut errs: Vec<String> = results.into_iter().filter_map(Result::err).collect();
    errs.sort_by_key(|e| e.starts_with("an earlier range"));
    if let Some(e) = errs.into_iter().next() {
        return Err(e);
    }
    let ok = store.insert_file(&part, &b.sha256, b.size).map_err(io)?;
    let _ = std::fs::remove_file(&marks);
    if !ok {
        // The bytes counted for this source were wrong: take them back out of the progress.
        done.fetch_sub(b.size, Ordering::SeqCst);
    }
    Ok(ok)
}

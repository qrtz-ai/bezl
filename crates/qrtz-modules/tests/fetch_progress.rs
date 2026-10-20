#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! N177 (ADR 0165): when the next source resumes a part a failed source started, the ranges that source finished
//! are not counted again. Echo's first Parakeet download reports this progress to the speech settings bar.
use qrtz_modules::fetch::{fetch_blobs, Source, Transport, RANGE_BYTES};
use qrtz_modules::index::BlobRef;
use qrtz_modules::store::Store;
use std::sync::Mutex;

const MIRROR: &str = "https://huggingface.co/m";

/// Host 0 serves the first `host0_ranges` ranges and fails the rest; host 1 fails everything; the mirror URL
/// serves all. With `host0_wrong`, host 0's bytes are the right length but wrong.
struct Flaky {
    data: Vec<u8>,
    host0_ranges: u64,
    host0_wrong: bool,
}

#[async_trait::async_trait]
impl Transport for Flaky {
    fn hosts(&self) -> usize {
        2
    }
    async fn get(&self, _host: usize, _path: &str, _max: usize) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    async fn get_range(&self, src: &Source, _sha256: &str, start: u64, len: u64) -> Result<Vec<u8>, String> {
        let bytes = self.data[start as usize..(start + len) as usize].to_vec();
        match src {
            Source::Host(0) if start / RANGE_BYTES < self.host0_ranges => {
                if self.host0_wrong {
                    Ok(bytes.iter().map(|b| b ^ 0xff).collect())
                } else {
                    Ok(bytes)
                }
            }
            Source::Host(_) => Err("connection reset".into()),
            Source::Url(_) => Ok(bytes),
        }
    }
}

/// Fetches one blob of `ranges` ranges (the last one short) and returns every `(done, total)` reported.
async fn run(ranges: u64, host0_ranges: u64, host0_wrong: bool) -> (Vec<(u64, u64)>, u64) {
    let size = (ranges - 1) * RANGE_BYTES + 12345;
    let data: Vec<u8> = (0..size).map(|i| (i * 31 % 251) as u8).collect();
    let h = qrtz_modules::sha256_hex(&data);
    let refs = vec![BlobRef {
        sha256: h.clone(),
        size,
        mirrors: vec![format!("{MIRROR}/{h}")],
    }];
    let t = Flaky {
        data,
        host0_ranges,
        host0_wrong,
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store"));
    let seen = Mutex::new(vec![]);
    fetch_blobs(&t, &store, &refs, &|d, tot| seen.lock().unwrap().push((d, tot)))
        .await
        .unwrap();
    assert!(store.verify(&h, size).unwrap());
    (seen.into_inner().unwrap(), size)
}

#[tokio::test]
async fn a_resumed_part_is_counted_once() {
    // Host 0 lands ranges 0 and 1, then drops; host 1 is down; Hugging Face resumes the part.
    let (seen, size) = run(6, 2, false).await;
    let max = seen.iter().map(|(d, _)| *d).max().unwrap();
    assert!(
        max <= size,
        "progress ran past the total: {max} > {size} ({seen:?})"
    );
    assert_eq!(seen.last().unwrap().0, size);
}

#[tokio::test]
async fn progress_never_goes_backwards_on_a_source_switch() {
    let (seen, _) = run(6, 3, false).await;
    for w in seen.windows(2) {
        assert!(w[1].0 >= w[0].0, "progress went back: {seen:?}");
    }
}

#[tokio::test]
async fn wrong_bytes_from_a_source_are_taken_back_out() {
    // Guard: every range from host 0 has the right length but the wrong bytes, so the part fails its hash and the
    // mirror starts over. The bar may go back, but it ends at exactly the total and never passes it.
    let (seen, size) = run(4, 4, true).await;
    let max = seen.iter().map(|(d, _)| *d).max().unwrap();
    assert!(max <= size, "{max} > {size}");
    assert_eq!(seen.last().unwrap().0, size);
}

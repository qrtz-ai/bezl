//! WAV header repair and part stitching (apps/desktop/src/worker/wav-repair.ts and `note.stepFinalize`).

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::js::js_round;

/// Fixes the RIFF/data sizes of a canonical 44-byte-header PCM WAV whose header was never finalized (helper
/// crashed mid-recording). Returns the duration in ms, or `None` if the file is missing or not such a WAV.
pub fn repair_wav(path: impl AsRef<Path>) -> Option<i64> {
    let mut f = OpenOptions::new().read(true).write(true).open(path).ok()?;
    let size = f.metadata().ok()?.len();
    if size < 44 {
        return None;
    }
    let mut h = [0u8; 44];
    f.read_exact(&mut h).ok()?;
    if &h[0..4] != b"RIFF" || &h[8..12] != b"WAVE" || &h[36..40] != b"data" {
        return None;
    }
    let data = size - 44;
    let u32_at = |h: &[u8; 44], i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
    if u64::from(u32_at(&h, 40)) != data {
        h[4..8].copy_from_slice(&((36 + data) as u32).to_le_bytes());
        h[40..44].copy_from_slice(&(data as u32).to_le_bytes());
        f.seek(SeekFrom::Start(0)).ok()?;
        f.write_all(&h).ok()?;
    }
    let byte_rate = u32_at(&h, 28);
    (byte_rate != 0).then(|| js_round(data as f64 / f64::from(byte_rate) * 1000.0))
}

/// H7 (ADR 0032): the most silence one reconnect gap can add before a part: the reconnect window plus the start
/// ack. A gap is never longer, so this only bounds a wall clock that jumped forward while the helper was down.
/// Same value in TS (`STITCH_MAX_GAP_MS`, apps/desktop/src/worker/wav-repair.ts).
pub const STITCH_MAX_GAP_MS: i64 = 35_000;
/// Copy and padding block: constant memory however long the meeting (N44).
const CHUNK: usize = 1 << 20;

/// Byte rate and block align of a canonical 44-byte-header PCM WAV (`wavFormat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavFormat {
    pub byte_rate: u32,
    pub block_align: u16,
}

/// Reads [`WavFormat`] from a WAV header, or `None` when it is missing, not such a WAV, or has a zero rate.
pub fn wav_format(path: impl AsRef<Path>) -> Option<WavFormat> {
    let mut f = fs::File::open(path).ok()?;
    let mut h = [0u8; 44];
    f.read_exact(&mut h).ok()?;
    if &h[0..4] != b"RIFF" || &h[8..12] != b"WAVE" || &h[36..40] != b"data" {
        return None;
    }
    let byte_rate = u32::from_le_bytes([h[28], h[29], h[30], h[31]]);
    let block_align = u16::from_le_bytes([h[32], h[33]]);
    (byte_rate != 0 && block_align != 0).then_some(WavFormat {
        byte_rate,
        block_align,
    })
}

/// H7: bytes of zero PCM that put a part recorded from meeting time `offset_ms` at its place, when `have_bytes` of
/// audio data precede it. Block-aligned, never negative, at most [`STITCH_MAX_GAP_MS`] (`gapPadBytes` in TS).
pub fn gap_pad_bytes(offset_ms: i64, have_bytes: u64, f: WavFormat) -> u64 {
    let rate = i128::from(f.byte_rate);
    let align = i128::from(f.block_align);
    let at = |ms: i64| i128::from(ms.max(0)) * rate / (1000 * align) * align;
    let pad = (at(offset_ms) - i128::from(have_bytes)).min(at(STITCH_MAX_GAP_MS));
    pad.max(0) as u64
}

fn write_zeros(out: &mut fs::File, n: u64) -> std::io::Result<()> {
    let z = vec![0u8; CHUNK.min(n as usize)];
    let mut left = n;
    while left > 0 {
        let k = (z.len() as u64).min(left) as usize;
        out.write_all(&z[..k])?;
        left -= k as u64;
    }
    Ok(())
}

/// Finalize's audio half (`stitchParts` in TS): repair the first part, append every existing later part's PCM
/// (header stripped) via `<main>.stitch` + atomic rename, delete the parts, repair again. Returns the final
/// duration.
///
/// H7: with `offsets_ms` (one per part, meeting time, `PipelineContext::part_offsets_ms`) each later part is
/// preceded by the zero PCM of its reconnect gap ([`gap_pad_bytes`]). Without (older rows): back to back.
/// N44: streamed (copy + chunks), constant memory. N28: a failed stitch removes its temp, the first part intact.
pub fn stitch_parts(parts: &[String], offsets_ms: Option<&[i64]>) -> std::io::Result<Option<i64>> {
    let Some((main, rest)) = parts.split_first() else {
        return Ok(None);
    };
    repair_wav(main);
    let offsets = offsets_ms.filter(|o| o.len() == parts.len());
    let extra: Vec<(&String, Option<i64>)> = rest
        .iter()
        .enumerate()
        .map(|(i, f)| (f, offsets.map(|o| o[i + 1])))
        .filter(|(f, _)| Path::new(f).exists())
        .collect();
    if !extra.is_empty() && Path::new(main).exists() {
        for (f, _) in &extra {
            repair_wav(f);
        }
        let fmt = offsets.and_then(|_| wav_format(main));
        let tmp = format!("{main}.stitch");
        let stitched = (|| -> std::io::Result<()> {
            fs::copy(main, &tmp)?;
            let mut out = OpenOptions::new().append(true).open(&tmp)?;
            let mut have = out.metadata()?.len().saturating_sub(44);
            for (f, off) in &extra {
                if let (Some(fmt), Some(off)) = (fmt, off) {
                    let n = gap_pad_bytes(*off, have, fmt);
                    write_zeros(&mut out, n)?;
                    have += n;
                }
                let mut src = fs::File::open(f)?;
                if src.metadata()?.len() > 44 {
                    src.seek(SeekFrom::Start(44))?;
                    have += std::io::copy(&mut src, &mut out)?;
                }
            }
            drop(out);
            fs::rename(&tmp, main)
        })();
        if let Err(e) = stitched {
            // N28: a failed stitch (disk full, unreadable part) must not leave a meeting-sized `.stitch` behind.
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        for (f, _) in &extra {
            let _ = fs::remove_file(f);
        }
    }
    Ok(repair_wav(main))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn write_wav(p: &Path, samples: usize) {
        let mut h = Vec::with_capacity(44 + samples * 2);
        h.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&16000u32.to_le_bytes());
        h.extend_from_slice(&32000u32.to_le_bytes());
        h.extend_from_slice(&2u16.to_le_bytes());
        h.extend_from_slice(&16u16.to_le_bytes());
        h.extend_from_slice(b"data\0\0\0\0");
        h.resize(44 + samples * 2, 0);
        fs::write(p, h).unwrap();
    }

    #[test]
    fn repairs_and_stitches() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.wav");
        let b = d.path().join("a.p1.wav");
        write_wav(&a, 1600);
        write_wav(&b, 1600);
        assert_eq!(repair_wav(&b), Some(100));
        let parts = vec![a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned()];
        assert_eq!(stitch_parts(&parts, None).unwrap(), Some(200));
        assert!(!b.exists());
        assert_eq!(fs::metadata(&a).unwrap().len(), 44 + 2 * 1600 * 2);
        assert_eq!(stitch_parts(&parts, None).unwrap(), Some(200), "idempotent");
        assert_eq!(repair_wav(d.path().join("missing.wav")), None);
        fs::write(
            d.path().join("junk.wav"),
            b"not a wav at all, definitely not one, no.....",
        )
        .unwrap();
        assert_eq!(repair_wav(d.path().join("junk.wav")), None);
    }
}

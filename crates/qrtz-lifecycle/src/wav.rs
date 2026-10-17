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

/// Finalize's audio half: repair the first part, append every existing later part's PCM (header stripped) via
/// `<main>.stitch` + atomic rename, delete the parts, repair again. Returns the final duration. Idempotent:
/// re-running after a crash at any point either finds the parts (old main intact) or finds them gone.
pub fn stitch_parts(parts: &[String]) -> std::io::Result<Option<i64>> {
    let Some((main, rest)) = parts.split_first() else {
        return Ok(None);
    };
    repair_wav(main);
    let extra: Vec<&String> = rest.iter().filter(|f| Path::new(f).exists()).collect();
    if !extra.is_empty() && Path::new(main).exists() {
        for f in &extra {
            repair_wav(f);
        }
        let tmp = format!("{main}.stitch");
        {
            let mut out = fs::File::create(&tmp)?;
            out.write_all(&fs::read(main)?)?;
            for f in &extra {
                let b = fs::read(f)?;
                if b.len() > 44 {
                    out.write_all(&b[44..])?;
                }
            }
        }
        fs::rename(&tmp, main)?;
        for f in &extra {
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
        assert_eq!(stitch_parts(&parts).unwrap(), Some(200));
        assert!(!b.exists());
        assert_eq!(fs::metadata(&a).unwrap().len(), 44 + 2 * 1600 * 2);
        assert_eq!(stitch_parts(&parts).unwrap(), Some(200), "idempotent");
        assert_eq!(repair_wav(d.path().join("missing.wav")), None);
        fs::write(
            d.path().join("junk.wav"),
            b"not a wav at all, definitely not one, no.....",
        )
        .unwrap();
        assert_eq!(repair_wav(d.path().join("junk.wav")), None);
    }
}

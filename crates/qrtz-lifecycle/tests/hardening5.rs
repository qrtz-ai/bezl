//! Hardening cycle 5 (N28): a failed stitch removes its `<main>.stitch` temp and keeps the first part intact; a
//! meeting delete also takes a leftover `<id>.wav.stitch`. Mirrors apps/desktop/src/main/finalize.hardening5.test.ts.

use std::path::Path;

use qrtz_lifecycle::runtime::meeting_audio;
use qrtz_lifecycle::wav::stitch_parts;

fn wav(p: &Path, data: usize) {
    let mut h = Vec::with_capacity(44 + data);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&((36 + data) as u32).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&16000u32.to_le_bytes());
    h.extend_from_slice(&32000u32.to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(data as u32).to_le_bytes());
    h.resize(44 + data, 0);
    std::fs::write(p, h).unwrap();
}

#[test]
fn failed_stitch_leaves_no_temp_and_retries() {
    let d = tempfile::tempdir().unwrap();
    let main = d.path().join("m.wav");
    let part = d.path().join("m.p1.wav");
    wav(&main, 3200);
    std::fs::create_dir(&part).unwrap(); // unreadable as a file: the read fails after the temp was created
    let before = std::fs::read(&main).unwrap();
    let parts = vec![
        main.to_string_lossy().into_owned(),
        part.to_string_lossy().into_owned(),
    ];
    assert!(stitch_parts(&parts, None).is_err());
    assert!(!d.path().join("m.wav.stitch").exists());
    assert_eq!(std::fs::read(&main).unwrap(), before);
    std::fs::remove_dir(&part).unwrap();
    wav(&part, 1600);
    assert_eq!(stitch_parts(&parts, None).unwrap(), Some(150));
    assert!(!part.exists());
}

#[test]
fn meeting_audio_takes_a_leftover_stitch_temp() {
    let d = tempfile::tempdir().unwrap();
    for f in ["m.wav", "m.wav.stitch", "m.p2.wav", "mm.wav.stitch"] {
        std::fs::write(d.path().join(f), b"").unwrap();
    }
    let mut got: Vec<String> = meeting_audio(d.path(), "m", &[])
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    got.sort();
    assert_eq!(got, vec!["m.p2.wav", "m.wav", "m.wav.stitch"]);
}

//! Hardening cycle 9 (H7, N44; ADR 0032): finalize pads each reconnect gap with zero PCM from the part offsets, so
//! audio time matches transcript time. Shared cases: spec/stress/stitch-gaps.json (TS:
//! apps/desktop/src/worker/stitch.hardening9.test.ts). Each part is filled with its own byte, so the layout is
//! checked, not only the length.

use std::fs;
use std::path::{Path, PathBuf};

use qrtz_lifecycle::wav::{gap_pad_bytes, stitch_parts, WavFormat, STITCH_MAX_GAP_MS};
use serde_json::Value;

fn spec() -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/stress/stitch-gaps.json");
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

fn fmt(sp: &Value) -> WavFormat {
    WavFormat {
        byte_rate: sp["format"]["byteRate"].as_u64().unwrap() as u32,
        block_align: sp["format"]["blockAlign"].as_u64().unwrap() as u16,
    }
}

/// A 44-byte-header 16-bit PCM WAV whose data is `fill` repeated; the header says 0 bytes (helper crashed).
fn write_part(p: &Path, sp: &Value, data: usize, fill: u8) {
    let f = fmt(sp);
    let mut h = Vec::with_capacity(44 + data);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&36u32.to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&(sp["format"]["sampleRate"].as_u64().unwrap() as u32).to_le_bytes());
    h.extend_from_slice(&f.byte_rate.to_le_bytes());
    h.extend_from_slice(&f.block_align.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&0u32.to_le_bytes());
    h.resize(44 + data, fill);
    fs::write(p, h).unwrap();
}

fn files(d: &Path, n: usize) -> Vec<PathBuf> {
    (0..n)
        .map(|i| {
            d.join(if i == 0 {
                "m.wav".to_string()
            } else {
                format!("m.p{i}.wav")
            })
        })
        .collect()
}

fn runs(b: &[u8]) -> Vec<(u8, usize)> {
    let mut out: Vec<(u8, usize)> = Vec::new();
    for &x in &b[44..] {
        match out.last_mut() {
            Some((v, n)) if *v == x => *n += 1,
            _ => out.push((x, 1)),
        }
    }
    out
}

fn strs(p: &[PathBuf]) -> Vec<String> {
    p.iter().map(|x| x.to_string_lossy().into_owned()).collect()
}

#[test]
fn cap_matches_the_spec() {
    assert_eq!(STITCH_MAX_GAP_MS, spec()["maxGapMs"].as_i64().unwrap());
}

#[test]
fn stitch_gap_cases_match_ts() {
    let sp = spec();
    let f = fmt(&sp);
    for c in sp["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let d = tempfile::tempdir().unwrap();
        let ps = c["parts"].as_array().unwrap();
        let parts = files(d.path(), ps.len());
        for (i, p) in ps.iter().enumerate() {
            if !p["missing"].as_bool().unwrap_or(false) {
                write_part(
                    &parts[i],
                    &sp,
                    p["dataBytes"].as_u64().unwrap() as usize,
                    (i + 1) as u8,
                );
            }
        }
        // `de_ms` rounding, as a persisted context is read.
        let offsets: Option<Vec<i64>> = c["offsetsMs"]
            .as_array()
            .map(|v| v.iter().map(|x| x.as_f64().unwrap().round() as i64).collect());
        let lined_up = offsets.as_ref().filter(|o| o.len() == ps.len());
        let mut have = ps[0]["dataBytes"].as_u64().unwrap();
        let mut pads: Vec<Value> = Vec::new();
        for (k, p) in ps.iter().enumerate().skip(1) {
            if p["missing"].as_bool().unwrap_or(false) {
                pads.push(Value::Null);
                continue;
            }
            let pad = lined_up.map_or(0, |o| gap_pad_bytes(o[k], have, f));
            have += pad + p["dataBytes"].as_u64().unwrap();
            pads.push(pad.into());
        }
        assert_eq!(Value::Array(pads), c["expect"]["padBytes"], "{name}: pads");

        let expect_ms = c["expect"]["durationMs"].as_i64().unwrap();
        let data = c["expect"]["dataBytes"].as_u64().unwrap() as usize;
        assert_eq!(
            stitch_parts(&strs(&parts), offsets.as_deref()).unwrap(),
            Some(expect_ms),
            "{name}"
        );
        let out = fs::read(&parts[0]).unwrap();
        assert_eq!(out.len(), 44 + data, "{name}: length");
        assert_eq!(
            u32::from_le_bytes(out[40..44].try_into().unwrap()) as usize,
            data,
            "{name}: header"
        );
        let mut want: Vec<(u8, usize)> = Vec::new();
        let mut push = |v: u8, n: usize| {
            if n == 0 {
                return;
            }
            match want.last_mut() {
                Some((x, m)) if *x == v => *m += n,
                _ => want.push((v, n)),
            }
        };
        push(1, ps[0]["dataBytes"].as_u64().unwrap() as usize);
        for (k, p) in ps.iter().enumerate().skip(1) {
            if p["missing"].as_bool().unwrap_or(false) {
                continue;
            }
            push(0, c["expect"]["padBytes"][k - 1].as_u64().unwrap() as usize);
            push((k + 1) as u8, p["dataBytes"].as_u64().unwrap() as usize);
        }
        assert_eq!(runs(&out), want, "{name}: layout");
        let left: Vec<String> = fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["m.wav"], "{name}: parts and temps removed");
        assert_eq!(
            stitch_parts(&strs(&parts), offsets.as_deref()).unwrap(),
            Some(expect_ms),
            "{name}: idempotent"
        );
        assert_eq!(fs::read(&parts[0]).unwrap(), out, "{name}: idempotent bytes");
    }
}

#[test]
fn disk_full_while_padding_keeps_the_first_part_and_retries() {
    let sp = spec();
    let d = tempfile::tempdir().unwrap();
    let parts = files(d.path(), 2);
    write_part(&parts[0], &sp, 32000, 1);
    fs::create_dir(&parts[1]).unwrap(); // unreadable as a file: the copy fails after the temp was written
    let before = fs::read(&parts[0]).unwrap();
    assert!(stitch_parts(&strs(&parts), Some(&[0, 3000])).is_err());
    assert!(!d.path().join("m.wav.stitch").exists());
    assert_eq!(fs::read(&parts[0]).unwrap()[44..], before[44..]);
    fs::remove_dir(&parts[1]).unwrap();
    write_part(&parts[1], &sp, 32000, 2);
    assert_eq!(stitch_parts(&strs(&parts), Some(&[0, 3000])).unwrap(), Some(4000));
    assert_eq!(
        runs(&fs::read(&parts[0]).unwrap()),
        vec![(1, 32000), (0, 64000), (2, 32000)]
    );
}

/// N41 (ADR 0033): the stitched bytes for two parts with a 1 s gap, from a clean run in its own dir.
fn reference(sp: &Value) -> Vec<u8> {
    let d = tempfile::tempdir().unwrap();
    let ps = files(d.path(), 2);
    write_part(&ps[0], sp, 32000, 1);
    write_part(&ps[1], sp, 16000, 2);
    stitch_parts(&strs(&ps), Some(&[0, 2000])).unwrap();
    fs::read(&ps[0]).unwrap()
}

fn names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn interrupted_stitch_finishes_without_duplicating_audio() {
    let sp = spec();
    let want = reference(&sp);
    // 0: died after the commit, before the parts were removed; 1: after the parts were removed, before the final
    // rename; 2: while writing `.stitch` (a partial temp).
    for stage in 0..3 {
        let d = tempfile::tempdir().unwrap();
        let ps = files(d.path(), 2);
        write_part(&ps[0], &sp, 32000, 1);
        write_part(&ps[1], &sp, 16000, 2);
        let main = ps[0].to_string_lossy().into_owned();
        match stage {
            0 => fs::write(format!("{main}.stitched"), &want).unwrap(),
            1 => {
                fs::remove_file(&ps[1]).unwrap();
                fs::write(format!("{main}.stitched"), &want).unwrap();
            }
            _ => fs::write(format!("{main}.stitch"), vec![7u8; 9999]).unwrap(),
        }
        assert_eq!(
            stitch_parts(&strs(&ps), Some(&[0, 2000])).unwrap(),
            Some(2500),
            "stage {stage}"
        );
        assert_eq!(fs::read(&ps[0]).unwrap(), want, "stage {stage}");
        assert_eq!(names(d.path()), vec!["m.wav"], "stage {stage}");
    }
}

//! Hardening cycle 36, ADR 0112: the `talk latency` line says how loud the mic was. The Linux cross-check
//! (docs/echo/dictation-tuning.md §10) measured that a quiet mic (speech 20 dB down) costs about a quarter more
//! word errors and a 15 dB room noise about a third more; whether Jared's mics are in that range decides whether
//! the helper's input normaliser (plan rank 7) is worth building. The helper already sends `dictation.level`
//! (block RMS × 4, clamped to 1) while a dictation listens; talk now keeps a 1 dB histogram of it per dictation and
//! logs `level_p10_dbfs` (the floor between words), `level_p90_dbfs` (about the speech level) and `level_blocks`.
mod common;
use common::*;
use qrtz_talk::level::*;
use qrtz_talk::*;
use std::sync::{Arc, Mutex};

#[test]
fn helper_levels_map_to_dbfs() {
    // rms × 4: 0.4 is an RMS of 0.1, -20 dBFS; 1 is the clamp, -12.04 dBFS; 0 is digital silence.
    assert!((helper_level_dbfs(0.4).unwrap() + 20.0).abs() < 1e-9);
    assert!((helper_level_dbfs(0.004).unwrap() + 60.0).abs() < 1e-9);
    assert!((helper_level_dbfs(1.0).unwrap() - CEIL_DBFS).abs() < 1e-9);
    assert_eq!(helper_level_dbfs(0.0), Some(f64::from(FLOOR_DBFS)));
    assert_eq!(helper_level_dbfs(f64::NAN), None);
    assert_eq!(
        helper_level_dbfs(7.0),
        helper_level_dbfs(1.0),
        "clamped like the helper"
    );
}

#[test]
fn the_meter_reports_the_floor_and_the_speech_level() {
    let mut m = LevelMeter::default();
    assert_eq!(m.summary(), None, "no level, nothing on the line");
    // 3 speech blocks at -20 dBFS for each pause block at -60 dBFS.
    for _ in 0..25 {
        for l in [0.4, 0.4, 0.4, 0.004] {
            m.add(l);
        }
    }
    m.add(f64::NAN);
    let s = m.summary().unwrap();
    assert_eq!(s.blocks, 100);
    assert_eq!(s.p10_dbfs, -60);
    assert_eq!(s.p90_dbfs, -20);
    assert_eq!(s.saturated, 0);
}

#[test]
fn the_meter_is_bounded_and_counts_the_clamp() {
    let mut m = LevelMeter::default();
    for i in 0..100_000u32 {
        m.add(if i % 10 == 0 { 1.0 } else { 0.0 });
    }
    let s = m.summary().unwrap();
    assert_eq!(s.blocks, 100_000);
    assert_eq!(s.saturated, 10_000);
    assert_eq!(s.p10_dbfs, FLOOR_DBFS);
    assert_eq!(s.p90_dbfs, FLOOR_DBFS, "90 % of the blocks are silence");
    assert_eq!(
        std::mem::size_of::<LevelMeter>(),
        4 * 91,
        "fixed size, however long the hold"
    );
}

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn latency_lines(buf: &Buf) -> Vec<String> {
    String::from_utf8(buf.0.lock().unwrap().clone())
        .unwrap()
        .lines()
        .filter(|l| l.contains("talk latency"))
        .map(str::to_owned)
        .collect()
}

fn field(line: &str, key: &str) -> Option<i64> {
    let at = line.find(&format!(" {key}="))?;
    let rest = &line[at + key.len() + 2..];
    let end = rest.find(' ').unwrap_or(rest.len());
    rest[..end].parse().ok()
}

async fn dictate(r: &Rig, hold_ms: u64) {
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(hold_ms).await;
    r.helper.key("up");
    until(
        || r.view().phase == TalkPhase::Idle && r.view().last.is_some(),
        10_000,
    )
    .await;
}

/// Two dictations; each line carries that dictation's levels only.
#[tokio::test(start_paused = true)]
async fn the_latency_line_carries_this_dictations_levels() {
    let buf = Buf::default();
    let w = buf.clone();
    let sub = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || w.clone())
        .finish();
    let _g = tracing::subscriber::set_default(sub);
    let r = rig(RigOpts {
        helper: HelperOpts {
            audio_every_ms: 100,
            levels: vec![0.4, 0.4, 0.4, 0.004],
            ..Default::default()
        },
        ..Default::default()
    });
    dictate(&r, 2000).await;
    dictate(&r, 2000).await;
    let lines = latency_lines(&buf);
    assert_eq!(lines.len(), 2, "{lines:?}");
    for l in &lines {
        assert_eq!(field(l, "level_p90_dbfs"), Some(-20), "{l}");
        assert_eq!(field(l, "level_p10_dbfs"), Some(-60), "{l}");
        let n = field(l, "level_blocks").expect(l);
        assert!(
            (15..=22).contains(&n),
            "one 2 s hold's blocks, not both dictations': {l}"
        );
        assert!(!l.contains("level_saturated"), "only when it happens: {l}");
    }
}

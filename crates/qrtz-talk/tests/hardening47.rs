#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Hardening cycle 47, ADR 0156 (N175): the `talk latency` line's stage times are plain numbers. They were logged
//! with `?`, so the line read `first_partial_ms=Some(890.4) final_ms=Some(1312.7) cleanup_ms=None`, unlike every other
//! field on it and unlike docs/echo/dictation-tuning.md §5. `scripts/echo-timing-report.mjs` and a plain
//! `grep … | awk` read numbers; now each stage time is whole ms, and a stage that never happened is left out.
mod common;
use common::*;
use qrtz_talk::*;
use std::sync::{Arc, Mutex};

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

fn raw_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let at = line.find(&format!(" {key}="))?;
    let rest = &line[at + key.len() + 2..];
    Some(&rest[..rest.find(' ').unwrap_or(rest.len())])
}

#[tokio::test(start_paused = true)]
async fn stage_times_are_plain_whole_ms() {
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
            ..Default::default()
        },
        ..Default::default()
    });
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(1500).await;
    r.helper.key("up");
    until(
        || r.view().phase == TalkPhase::Idle && r.view().last.is_some(),
        10_000,
    )
    .await;
    let lines = latency_lines(&buf);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let l = &lines[0];
    assert!(!l.contains("Some(") && !l.contains("=None"), "{l}");
    let mut seen = 0;
    for key in ["first_partial_ms", "final_ms", "cleanup_ms", "inserted_ms"] {
        if let Some(v) = raw_field(l, key) {
            let n: f64 = v
                .parse()
                .unwrap_or_else(|_| panic!("{key}={v} isn't a number: {l}"));
            assert_eq!(n, n.round(), "{key} is whole ms: {l}");
            seen += 1;
        }
    }
    assert!(
        raw_field(l, "final_ms").is_some(),
        "a finished dictation has its final: {l}"
    );
    assert!(seen >= 2, "{l}");
}

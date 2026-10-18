//! Hardening cycle 43, ADR 0132 (ADR 0131's release tail confidence floor, N169): `dictation.stopped` may carry the
//! release tail decode's confidence (`tailConf`) and whether the helper's floor dropped that tail (`tailDropped`).
//! Talk parses both (additive: an older helper sends neither) and logs them on the `talk latency` line, beside
//! `raw_chars` / `inserted_chars`, so a dictation that lost its last words to the floor shows on the same line as
//! what was inserted. Out-of-range or non-numeric confidences are dropped, never logged.
mod common;
use common::*;
use qrtz_talk::*;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[test]
fn the_tail_fields_parse() {
    let a = AsrInfo::from_stopped(&json!({"asr": "batch-v2", "segments": 2, "tailDecodeMs": 80.0,
        "tailConf": 0.4321, "tailDropped": true}))
    .unwrap();
    assert_eq!(
        a.tail_conf,
        Some(0.432),
        "rounded to 3 places like the helper's ms fields"
    );
    assert!(a.tail_dropped);
    let kept =
        AsrInfo::from_stopped(&json!({"asr": "p", "segments": 2, "tailConf": 0.97, "tailDropped": false}))
            .unwrap();
    assert_eq!((kept.tail_conf, kept.tail_dropped), (Some(0.97), false));
    let old = AsrInfo::from_stopped(&json!({"asr": "p", "segments": 2})).unwrap();
    assert_eq!(
        (old.tail_conf, old.tail_dropped),
        (None, false),
        "an older helper: neither"
    );
    for bad in [json!(-0.1), json!(1.5), json!("0.5"), json!(null)] {
        let b = AsrInfo::from_stopped(&json!({"asr": "p", "tailConf": bad, "tailDropped": "yes"})).unwrap();
        assert_eq!((b.tail_conf, b.tail_dropped), (None, false), "{bad}");
    }
    let edge = AsrInfo::from_stopped(&json!({"asr": "p", "tailConf": 1.0})).unwrap();
    assert_eq!(edge.tail_conf, Some(1.0));
    let zero = AsrInfo::from_stopped(&json!({"asr": "p", "tailConf": 0})).unwrap();
    assert_eq!(zero.tail_conf, Some(0.0));
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

/// End to end: a dropped tail logs `tail_conf` and `tail_dropped=true`; a kept one only its `tail_conf`; an older
/// helper neither.
#[tokio::test(start_paused = true)]
async fn the_latency_line_says_when_the_tail_was_dropped() {
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
            stopped_extra: Some(json!({"asr": "batch-v2", "segments": 2, "tailDecodeMs": 80.0,
                "tailConf": 0.41, "tailDropped": true})),
            ..Default::default()
        },
        ..Default::default()
    });
    dictate(&r, 2000).await;
    r.helper.set_opts(|o| {
        o.stopped_extra =
            Some(json!({"asr": "batch-v2", "segments": 2, "tailConf": 0.93, "tailDropped": false}))
    });
    dictate(&r, 2000).await;
    r.helper
        .set_opts(|o| o.stopped_extra = Some(json!({"asr": "batch-v2", "segments": 2})));
    dictate(&r, 2000).await;
    let lines = latency_lines(&buf);
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(
        lines[0].contains(" tail_conf=0.41") && lines[0].contains(" tail_dropped=true"),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].contains(" tail_conf=0.93") && !lines[1].contains("tail_dropped"),
        "{}",
        lines[1]
    );
    assert!(
        !lines[2].contains("tail_conf") && !lines[2].contains("tail_dropped"),
        "{}",
        lines[2]
    );
}

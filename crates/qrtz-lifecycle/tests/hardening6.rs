#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Hardening cycle 6 stress (spec/stress/capture-bounds.json, also run by
//! packages/core/src/note/lifecycle.hardening6.test.ts): hours of a flapping capture source and a fallback loop with
//! huge error text keep the session row and the capture report bounded (N29/H8 limits), and every cut is code-point
//! safe. Events are built from JSON, so the wire names are exercised exactly as TS writes them.

use qrtz_lifecycle::model::{LifecycleEvent, RecordingState};
use qrtz_lifecycle::{capture_limits, capture_report, initial_state, reduce};
use serde_json::{json, Value};

fn spec() -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/stress/capture-bounds.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn step(s: RecordingState, e: Value) -> RecordingState {
    let e: LifecycleEvent = serde_json::from_value(e).unwrap();
    reduce(&s, &e).unwrap_or(s)
}

fn run(sp: &Value) -> RecordingState {
    let t0 = sp["startedAt"].as_i64().unwrap();
    let period = sp["periodMs"].as_i64().unwrap();
    let quiet = sp["silentForMs"].as_i64().unwrap();
    let src = sp["source"].clone();
    let mut s = step(
        initial_state(),
        json!({"type": "start-requested", "meetingId": "m1", "at": t0, "audioPath": "/r/m1.wav"}),
    );
    s = step(s, json!({"type": "start-acked"}));
    for i in 0..sp["flaps"].as_i64().unwrap() {
        let since = i * period;
        s = step(
            s,
            json!({"type": "input-silent", "source": src, "sinceMs": since}),
        );
        s = step(
            s,
            json!({"type": "input-audible", "source": src, "at": t0 + since + quiet}),
        );
    }
    let reason = sp["reasonUnit"]
        .as_str()
        .unwrap()
        .repeat(sp["reasonRepeat"].as_u64().unwrap() as usize);
    for i in 0..sp["fallbacks"].as_i64().unwrap() {
        let to = if i % 2 == 1 {
            json!(reason)
        } else {
            json!("screencapturekit")
        };
        s = step(
            s,
            json!({"type": "capture-fallback", "from": reason, "to": to, "reason": reason, "at": t0 + i}),
        );
    }
    s
}

#[test]
fn limits_match_the_spec() {
    let x = &spec()["expect"];
    assert_eq!(
        capture_limits::SILENT_SPANS as u64,
        x["silentSpans"].as_u64().unwrap()
    );
    assert_eq!(capture_limits::FALLBACKS as u64, x["fallbacks"].as_u64().unwrap());
    assert_eq!(
        capture_limits::REASON_CHARS as u64,
        x["reasonCodePoints"].as_u64().unwrap()
    );
}

#[test]
fn flapping_keeps_the_oldest_stretches_and_the_row_small() {
    let sp = spec();
    let x = &sp["expect"];
    let s = run(&sp);
    assert_eq!(s.silent_spans.len() as u64, x["silentSpans"].as_u64().unwrap());
    assert_eq!(serde_json::to_value(&s.silent_spans[0]).unwrap(), x["first"]);
    assert_eq!(
        serde_json::to_value(s.silent_spans.last().unwrap()).unwrap(),
        x["last"]
    );
    assert!(s.silent.is_empty());
    let bytes = serde_json::to_string(&s).unwrap().len() as u64;
    assert!(bytes < x["maxSessionJsonBytes"].as_u64().unwrap(), "{bytes}");
}

#[test]
fn fallback_loop_keeps_eight_cut_by_code_points() {
    let sp = spec();
    let x = &sp["expect"];
    let s = run(&sp);
    assert_eq!(s.fallbacks.len() as u64, x["fallbacks"].as_u64().unwrap());
    let max = x["reasonCodePoints"].as_u64().unwrap() as usize;
    let unit = sp["reasonUnit"].as_str().unwrap();
    let want: String = unit.repeat(400).chars().take(max).collect();
    for f in &s.fallbacks {
        assert_eq!(f.reason, want, "same cut as TS Array.from(v).slice(0, n)");
        assert_eq!(f.from.chars().count(), max);
        assert!(f.to.as_deref().is_none_or(|t| t.chars().count() <= max));
    }
}

#[test]
fn a_stretch_open_at_a_crash_is_still_capped_in_the_report() {
    let sp = spec();
    let x = &sp["expect"];
    let s = step(
        run(&sp),
        json!({"type": "input-silent", "source": "system", "sinceMs": 1}),
    );
    assert_eq!(s.silent.len(), 1);
    let r = capture_report(&s).unwrap();
    assert_eq!(
        r.silent_spans.len() as u64,
        x["reportSpansWithOpen"].as_u64().unwrap()
    );
    assert_eq!(r.fallbacks.len() as u64, x["fallbacks"].as_u64().unwrap());
}

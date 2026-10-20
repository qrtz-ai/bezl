#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Hardening cycle 30 (N150, ADR 0096): the pill gate over refusals as `qrtz-talk` now emits them. A press refused
//! before `dictation.start` (speech module missing, stale, revoked) carries the id that press drew, so tapping fn
//! again after a refusal flashes again, with no idle view in between (talk emits none: the key press itself emits
//! nothing, the refusal is the next view). A re-emitted view keeps its id and does not flash twice.
use qrtz_app::pill::{PillAction, PillGate};
use serde_json::{json, Value};

fn refused(id: Option<&str>) -> Value {
    let mut v = json!({
        "phase": "error",
        "error": "speech isn't ready yet",
        "errorKind": "speech-unavailable",
        "last": {"id": "t1", "inserted": true},
    });
    if let Some(id) = id {
        v["id"] = json!(id);
    }
    v
}

fn flashes(g: &mut PillGate, v: &Value) -> bool {
    matches!(g.next(&json!({}), v), PillAction::Flash { .. })
}

#[test]
fn each_refused_tap_flashes_once_even_back_to_back() {
    let mut g = PillGate::default();
    assert!(flashes(&mut g, &refused(Some("t2"))), "the first refusal flashes");
    assert!(
        !flashes(&mut g, &refused(Some("t2"))),
        "a re-emitted view doesn't"
    );
    assert!(
        flashes(&mut g, &refused(Some("t3"))),
        "the next tap's refusal flashes"
    );
    assert!(flashes(&mut g, &refused(Some("t4"))), "and the one after");
}

#[test]
fn a_refusal_with_no_attempt_id_is_the_shape_that_went_silent() {
    // The shape before N150 (and the push-key-watch refusal, which is not a press): back to back, the gate can't
    // tell a second tap from a re-emit, so it flashes once. Kept to pin why the press's id matters.
    let mut g = PillGate::default();
    assert!(flashes(&mut g, &refused(None)));
    assert!(!flashes(&mut g, &refused(None)));
}

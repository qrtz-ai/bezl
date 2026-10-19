//! Echo cycle 73, N195 (ADR 0224): `error{dictation.mic}` may say why the mic didn't open (`reason`). A start
//! refused because the input device isn't there (`no-device`) is mic-lost, not "microphone access denied";
//! `denied` or no reason (an older helper) stays mic-denied with the helper's message.
mod common;
use common::*;
use qrtz_talk::service::mic_error;
use qrtz_talk::*;
use serde_json::json;

#[test]
fn the_reason_maps_to_the_error_kind() {
    let e = |r: Option<&str>| {
        let mut m = json!({"type": "error", "code": "dictation.mic", "message": "x", "fatal": false});
        if let Some(r) = r {
            m["reason"] = json!(r);
        }
        mic_error(&m)
    };
    assert_eq!(e(Some("no-device")).0, TalkErrorKind::MicLost);
    assert_eq!(e(Some("denied")), (TalkErrorKind::MicDenied, "x".to_owned()));
    assert_eq!(e(None), (TalkErrorKind::MicDenied, "x".to_owned()));
    assert_eq!(e(Some("something-new")).0, TalkErrorKind::MicDenied);
}

/// The helper refuses this press's start by its id: no input device. Talk says the mic is gone and cancels it.
#[tokio::test(start_paused = true)]
async fn a_start_refused_for_no_device_is_mic_lost() {
    let r = rig(RigOpts::default());
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    let id = r.view().id.clone().expect("an id");
    r.helper.deliver(
        json!({"type": "error", "code": "dictation.mic", "message": "no microphone input device",
                            "fatal": false, "id": id, "reason": "no-device"}),
    );
    until(|| r.view().phase == TalkPhase::Error, 1000).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::MicLost));
    assert_eq!(
        r.helper
            .sent_of("dictation.cancel")
            .iter()
            .filter(|m| m["id"] == id.as_str())
            .count(),
        1
    );
    r.helper.key("up");
}

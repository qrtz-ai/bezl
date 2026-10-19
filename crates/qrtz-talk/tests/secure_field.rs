//! ADR 0225 (Bozniak, echo 0.2.1 blocker): echo must never type into a password field. The helper checks the focused
//! field right at insert time and answers `insert.result{ok: false, error: "secure-field"}`; the host then keeps
//! nothing of that dictation (no history, no clipboard, no transcript) and the pill says why.
mod common;
use common::*;
use qrtz_talk::*;

#[tokio::test(start_paused = true)]
async fn a_secure_refusal_keeps_nothing_and_says_so() {
    let r = rig(RigOpts {
        helper: HelperOpts {
            insert_secure: true,
            audio_every_ms: 50,
            ..Default::default()
        },
        ..Default::default()
    });
    let v = r.dictate(400).await;
    sleep(500).await;
    assert_eq!(
        r.helper.sent_of("insert.text").len(),
        1,
        "the helper was asked, and refused"
    );
    assert!(v.last.is_none(), "no entry in the view: {:?}", v.last);
    assert!(v.text.is_empty() && v.error.is_none(), "{v:?}");
    assert!(r.svc.history().is_empty(), "no history");
    assert!(r.clip.0.lock().unwrap().is_empty(), "nothing on the clipboard");
    assert!(
        !r.events
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|(e, _)| e == "talk:history-changed"),
        "no history event"
    );
    let typed = r.helper.sent_of("insert.text")[0]["text"]
        .as_str()
        .unwrap()
        .trim()
        .to_owned();
    assert!(!typed.is_empty());
    if let Ok(bytes) = std::fs::read(&r.store) {
        assert!(
            !String::from_utf8_lossy(&bytes).contains(&typed),
            "nothing stored"
        );
    }
    let f = r.helper.sent_of("pill.flash");
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["kind"], "info");
    assert_eq!(f[0]["message"], MSG_SECURE_FIELD);
}

#[test]
fn the_notice_is_lowercase() {
    assert_eq!(MSG_SECURE_FIELD, "echo won't type into password fields");
    assert_eq!(INSERT_REFUSED_SECURE, "secure-field");
}

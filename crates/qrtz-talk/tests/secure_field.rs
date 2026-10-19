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

fn slow_final(stopped_delay_ms: u64, insert_delay_ms: u64, insert: u64) -> RigOpts {
    RigOpts {
        helper: HelperOpts {
            insert_secure: true,
            audio_every_ms: 100,
            stopped_delay_ms,
            insert_delay_ms,
            final_tail_words: 3,
            ..Default::default()
        },
        timeouts: Timeouts {
            stopped: 500,
            stopped_per_audio_s: 0,
            stopped_max: 500,
            helper_tail: None,
            insert,
            ..Timeouts::default()
        },
        ..Default::default()
    }
}

async fn hold(r: &Rig, ms: u64) {
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(ms).await;
    r.helper.key("up");
}

fn kept_nothing(r: &Rig) {
    assert!(r.svc.history().is_empty(), "no history: {:?}", r.svc.history());
    assert!(r.view().last.is_none(), "no last entry");
    assert!(r.view().text.is_empty() && r.view().partial.is_empty());
    assert!(r.clip.0.lock().unwrap().is_empty(), "nothing on the clipboard");
}

/// ADR 0225 follow-up 5: the real final comes after the stop wait ran out and after the refusal: dropped (the N124
/// late path would otherwise put it on the clipboard, with no history entry to replace).
#[tokio::test(start_paused = true)]
async fn a_late_final_after_a_secure_refusal_is_dropped() {
    let r = rig(slow_final(1200, 0, 2500));
    hold(&r, 500).await;
    until(|| !r.view().active(), 5000).await;
    sleep(2000).await;
    assert_eq!(r.helper.finals().len(), 1, "the late final did come");
    kept_nothing(&r);
}

/// The late final lands while the refused insert is still in flight: dropped too.
#[tokio::test(start_paused = true)]
async fn a_late_final_during_a_refused_insert_is_dropped() {
    let r = rig(slow_final(800, 1500, 3000));
    hold(&r, 500).await;
    until(|| r.view().phase == TalkPhase::Inserting, 3000).await;
    until(|| !r.view().active(), 5000).await;
    sleep(2000).await;
    kept_nothing(&r);
}

/// ADR 0225 follow-up 1: the refusal comes after the insert bound ran out. talk went ahead as for a failed insert
/// (the words on the clipboard, an entry in history); the late refusal takes the entry back out and says why.
/// The clipboard write can't be taken back: the documented limit.
#[tokio::test(start_paused = true)]
async fn a_secure_refusal_after_the_insert_bound_removes_the_history_entry() {
    let mut o = slow_final(0, 2000, 500);
    o.timeouts = Timeouts {
        insert: 500,
        ..Timeouts::default()
    };
    let r = rig(o);
    let _ = r.dictate(400).await;
    assert_eq!(r.svc.history().len(), 1, "kept as a failed insert first");
    sleep(3000).await;
    assert!(r.svc.history().is_empty(), "taken back out");
    assert!(r.view().last.is_none());
    assert_eq!(
        r.clip.0.lock().unwrap().len(),
        1,
        "the documented limit: the clipboard write stays"
    );
    let f = r.helper.sent_of("pill.flash");
    assert_eq!(f.last().unwrap()["message"], MSG_SECURE_FIELD, "{f:?}");
}

/// Bozniak follow-up (a): with history off, a refusal after the insert bound has no entry to remove and nothing
/// takes its id back out of the remembered set: it stays capped, and the notice is still flashed each time.
#[tokio::test(start_paused = true)]
async fn refusals_after_the_bound_with_history_off_stay_capped_and_say_so() {
    let mut o = slow_final(0, 2000, 500);
    o.timeouts = Timeouts {
        insert: 500,
        ..Timeouts::default()
    };
    o.config = serde_json::json!({"talk": {"saveHistory": false}});
    let r = rig(o);
    let n = MAX_SECURE_REFUSED + 4;
    for _ in 0..n {
        let _ = r.dictate(300).await;
        sleep(3000).await;
    }
    assert!(r.svc.history().is_empty());
    assert!(
        r.svc.secure_refused_len() <= MAX_SECURE_REFUSED,
        "{}",
        r.svc.secure_refused_len()
    );
    let notices = r
        .helper
        .sent_of("pill.flash")
        .iter()
        .filter(|f| f["message"] == MSG_SECURE_FIELD && f["kind"] == "info")
        .count();
    assert_eq!(notices, n, "the notice every time");
}

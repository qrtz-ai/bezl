//! QRT-291: when the supervisor gave up on the capture helper (a refused launch), "the capture helper isn't
//! running" says why: the check that refused it, as the supervisor worded it (no path). Once it is retried, the
//! reason goes.
mod common;
use common::*;
use qrtz_talk::*;

#[tokio::test(start_paused = true)]
async fn a_press_while_the_helper_gave_up_says_why() {
    let r = rig(RigOpts::default());
    r.helper.die();
    r.svc.helper_refusal(Some(
        "echo: signature mismatch: the helper isn't signed with qrtz's Developer ID identity",
    ));
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Error, 2_000).await;
    let v = r.view();
    assert_eq!(v.error_kind, Some(TalkErrorKind::HelperUnavailable));
    assert_eq!(
        v.error.as_deref(),
        Some("the capture helper isn't running, so the mic can't open (echo: signature mismatch: the helper isn't signed with qrtz's Developer ID identity)")
    );
    r.helper.key("up");

    r.svc.helper_refusal(None);
    r.helper.key("down");
    sleep(20).await;
    until(|| r.view().phase == TalkPhase::Error, 2_000).await;
    assert_eq!(
        r.view().error.as_deref(),
        Some("the capture helper isn't running, so the mic can't open")
    );
}

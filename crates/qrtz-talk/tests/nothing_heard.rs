//! Oct 18 (Jared, the Developer ID build: "holding fn/globe does nothing"): a dictation that heard no words ended
//! with no line in the log and nothing on screen. Now the host logs the mic level (`talk: nothing heard`) and the
//! pill says why nothing was typed: no sound from the mic, or no words. A tap of the key says nothing.
mod common;
use common::*;
use qrtz_talk::level::LevelSummary;
use qrtz_talk::*;

fn lvl(p90: i32) -> Option<LevelSummary> {
    Some(LevelSummary {
        blocks: 20,
        p10_dbfs: p90 - 10,
        p90_dbfs: p90,
        saturated: 0,
    })
}

#[test]
fn message_by_level_and_length() {
    assert_eq!(nothing_heard(200.0, lvl(-30)), None, "a tap");
    assert_eq!(nothing_heard(f64::NAN, lvl(-30)), None);
    assert_eq!(nothing_heard(2000.0, lvl(-100)), Some(MSG_SILENT_MIC));
    assert_eq!(nothing_heard(2000.0, lvl(-90)), Some(MSG_SILENT_MIC));
    assert_eq!(nothing_heard(2000.0, lvl(-60)), Some(MSG_NOTHING_HEARD));
    assert_eq!(nothing_heard(2000.0, None), Some(MSG_NOTHING_HEARD));
    for m in [MSG_SILENT_MIC, MSG_NOTHING_HEARD] {
        assert_eq!(m, m.to_lowercase(), "ui text is lowercase");
        assert!(m.chars().count() <= 200);
    }
}

async fn hold_silently(levels: Vec<f64>) -> Vec<serde_json::Value> {
    let r = rig(RigOpts {
        helper: HelperOpts {
            audio_every_ms: 100,
            no_words: true,
            levels,
            ..Default::default()
        },
        ..Default::default()
    });
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(1500).await;
    r.helper.key("up");
    until(|| r.view().phase == TalkPhase::Idle, 10_000).await;
    sleep(50).await;
    assert!(r.helper.sent_of("insert.text").is_empty(), "nothing to type");
    r.helper.sent_of("pill.flash")
}

#[tokio::test(start_paused = true)]
async fn silent_mic_says_so_on_the_pill() {
    let f = hold_silently(vec![0.0]).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["kind"], "info");
    assert_eq!(f[0]["message"], MSG_SILENT_MIC);
}

#[tokio::test(start_paused = true)]
async fn no_words_says_so_on_the_pill() {
    let f = hold_silently(vec![]).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["message"], MSG_NOTHING_HEARD);
}

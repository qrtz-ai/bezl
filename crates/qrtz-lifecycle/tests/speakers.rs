#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Per-channel speakers through the real finalize and storage (ADR 0058 §12): ids stay per channel, the mic's
//! main voice is "me" by default, a rename sticks across another finalize, and the mic is never one person.
#![cfg(feature = "runtime")]
use qrtz_lifecycle::runtime::finalize_sync;
use qrtz_model::*;
use qrtz_storage::Db;

fn seg(id: &str, a: Ms, b: Ms, text: &str) -> Segment {
    Segment {
        id: id.into(),
        start_ms: a,
        end_ms: b,
        text: text.into(),
        speaker: None,
        speaker_name: None,
        source: SegmentSource::Live,
        confidence: None,
    }
}

fn turn(s: &str, a: Ms, b: Ms) -> SpeakerTurn {
    SpeakerTurn {
        speaker: s.into(),
        start_ms: a,
        end_ms: b,
    }
}

#[test]
fn finalize_keeps_per_channel_ids_the_me_default_and_renames() {
    let db = Db::open_in_memory().unwrap();
    let repo = db.notes();
    repo.upsert_meeting(&Meeting {
        id: "m1".into(),
        title: "Standup".into(),
        platform: MeetingPlatform::Other,
        started_at: 1,
        ended_at: None,
        participants: vec![],
        status: MeetingStatus::Recording,
        audio_path: None,
        template: None,
        duration_ms: None,
    })
    .unwrap();
    for s in [
        seg("u1", 0, 1000, "morning"),
        seg("u2", 1000, 2000, "can I add"),
        seg("system-u3", 2000, 3000, "sure"),
        seg("u4", 4000, 5000, "unattributed"),
    ] {
        repo.append_segment("m1", &s).unwrap();
    }
    let ctx = PipelineContext {
        started_at: 1,
        ended_at: 6000,
        duration_ms: 6000,
        audio_parts: vec![],
        part_offsets_ms: None,
        active: vec![],
        helper_crashed: false,
        speaker_turns: vec![
            turn("mic:0", 0, 1000),
            turn("mic:1", 1000, 2000),
            turn("system:0", 2000, 3000),
        ],
    };
    let ids = |db: &Db| -> Vec<(String, Option<String>, Option<String>)> {
        db.notes()
            .get_segments("m1")
            .unwrap()
            .into_iter()
            .map(|s| (s.id, s.speaker, s.speaker_name))
            .collect()
    };
    finalize_sync(&db, "m1", &ctx).unwrap();
    let o = |s: &str| Some(s.to_string());
    assert_eq!(
        ids(&db),
        [
            ("u1".into(), o("mic:0"), o("me")),
            ("u2".into(), o("mic:1"), o("speaker 1")),
            ("system-u3".into(), o("system:0"), o("speaker 2")),
            ("u4".into(), None, None),
        ]
    );
    // note:rename-speaker: "me" → "Jared", "speaker 1" → "Lee"; a re-run of finalize (retry) keeps both.
    db.notes().rename_speaker("m1", "me", "Jared").unwrap();
    db.notes().rename_speaker("m1", "speaker 1", "Lee").unwrap();
    finalize_sync(&db, "m1", &ctx).unwrap();
    assert_eq!(
        ids(&db),
        [
            ("u1".into(), o("mic:0"), o("Jared")),
            ("u2".into(), o("mic:1"), o("Lee")),
            ("system-u3".into(), o("system:0"), o("speaker 2")),
            ("u4".into(), None, None),
        ]
    );
}

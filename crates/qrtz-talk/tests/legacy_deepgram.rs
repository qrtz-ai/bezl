#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0067: Deepgram is gone. State written while it existed still loads, in qrtz-talk on its own (without
//! qrtz-app's `migrate_legacy`): a config that still says `talk.engine = deepgram` dictates on-device (talk has
//! no cloud engine at all since ADR 0076); a talk.json history entry from Deepgram keeps its text and reads as
//! `elevenlabs` (the read-only legacy cloud label, `store::legacy_entry`); everything else in the file is kept.
mod common;
use common::*;
use qrtz_talk::store::{load, parse, FsIo, LoadOutcome};
use qrtz_talk::*;
use serde_json::{json, Value};

#[tokio::test(start_paused = true)]
async fn an_unmigrated_deepgram_pick_dictates_on_device() {
    let r = rig(RigOpts {
        config: json!({"talk": {"engine": "deepgram", "autoFailover": false, "restoreClipboard": false}}),
        ..Default::default()
    });
    let cfg = TalkConfig::from_app_config(&r.config.lock().unwrap());
    assert!(!cfg.restore_clipboard, "the rest of the talk config still reads");
    assert_eq!(r.svc.resolve_engine(), Engine::OnDevice);
    let v = r.dictate(300).await;
    let e = v.last.expect("an entry");
    assert_eq!(e.engine, Engine::OnDevice);
    assert!(!e.failover);
}

#[test]
fn deepgram_no_longer_parses_as_an_engine() {
    assert!(serde_json::from_value::<Engine>(json!("deepgram")).is_err());
    assert_eq!(
        serde_json::from_value::<Engine>(json!("elevenlabs")).unwrap(),
        Engine::Elevenlabs
    );
}

fn entry(id: &str, engine: &str) -> Value {
    json!({"id": id, "at": 1760000000000u64, "raw": "x", "text": format!("{id} text."), "cleanup": "rules",
           "engine": engine, "failover": false, "insert": "paste", "inserted": true, "durationMs": 10,
           "latency": {}})
}

fn old_talk_json() -> Vec<u8> {
    json!({
        "history": [entry("on", "on-device"), entry("dg", "deepgram"), entry("el", "elevenlabs"),
                    entry("bad", "a-model-name")],
        "dictionary": [{"term": "qrtz", "aliases": ["quartz"]}],
    })
    .to_string()
    .into_bytes()
}

#[test]
fn an_old_talk_json_keeps_the_deepgram_entry_as_cloud_and_everything_else() {
    let (st, dropped) = parse(&old_talk_json()).expect("still a talk.json, not corrupt");
    assert_eq!(dropped, 1, "only the genuinely invalid engine is dropped");
    let got: Vec<(&str, Engine, &str)> = st
        .history
        .iter()
        .map(|h| (h.id.as_str(), h.engine, h.text.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("on", Engine::OnDevice, "on text."),
            ("dg", Engine::Elevenlabs, "dg text."),
            ("el", Engine::Elevenlabs, "el text."),
        ]
    );
    assert_eq!(st.dictionary.len(), 1);

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("talk.json");
    std::fs::write(&file, old_talk_json()).unwrap();
    let (st2, out) = load(&FsIo, &file, 1.0);
    assert_eq!(out, LoadOutcome::Loaded { dropped: 1 }, "never quarantined");
    assert_eq!(st2, st);
    assert!(file.exists());
}

#[tokio::test(start_paused = true)]
async fn the_service_starts_on_an_old_talk_json_and_rewrites_it_without_deepgram() {
    let r = rig(RigOpts {
        seed_store: Some(old_talk_json()),
        ..Default::default()
    });
    assert_eq!(r.svc.history().len(), 3);
    r.dictate(300).await;
    let h: Vec<String> = r.svc.history().into_iter().map(|e| e.id).collect();
    assert_eq!(h.len(), 4, "{h:?}");
    for id in ["on", "dg", "el"] {
        assert!(h.iter().any(|x| x == id), "{id} kept: {h:?}");
    }
    let body = String::from_utf8(std::fs::read(&r.store).unwrap()).unwrap();
    assert!(!body.contains("deepgram"), "{body}");
    assert!(
        body.contains("dg text."),
        "the Deepgram dictation's text survives: {body}"
    );
    assert!(body.contains("quartz"), "the dictionary survives: {body}");
}

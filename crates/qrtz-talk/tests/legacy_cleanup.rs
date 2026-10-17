//! ADR 0071: talk has no LLM cleanup. The final ASR text gets the on-device rules (or nothing, with
//! `talk.autoCleanup` off) and goes straight in. State written while the LLM cleanup existed still loads: a config
//! with `aiCleanup` / `cleanupModel` / `cleanupTimeoutMs` dictates with the rules and never waits on anything, and
//! a talk.json whose entries say `cleanup: llm` (cleaned `text`, original `raw`, `cleanupMs`) keeps them as written.
mod common;
use common::*;
use qrtz_talk::cleanup::{apply_dictionary, cleanup_dictation, rule_cleanup};
use qrtz_talk::contract::CleanupMethod;
use qrtz_talk::store::{load, parse, FsIo, LoadOutcome};
use qrtz_talk::*;
use serde_json::{json, Value};

fn old_cleanup_config(auto_cleanup: bool) -> Value {
    json!({
        "talk": {"autoCleanup": auto_cleanup, "aiCleanup": true, "cleanupModel": "anthropic/claude-haiku-5.5",
                 "cleanupTimeoutMs": 10_000, "restoreClipboard": false},
        "privacy": {"localOnly": false},
    })
}

#[test]
fn an_old_config_with_cleanup_settings_still_reads() {
    let cfg = TalkConfig::from_app_config(&old_cleanup_config(true));
    assert!(cfg.auto_cleanup);
    assert!(!cfg.restore_clipboard, "the rest of talk still reads");
    assert!(cfg.auto_insert);
}

#[tokio::test(start_paused = true)]
async fn an_old_ai_cleanup_config_dictates_with_the_rules_and_inserts_on_release() {
    let r = rig(RigOpts {
        config: old_cleanup_config(true),
        ..Default::default()
    });
    // An OpenRouter key saved for notes: talk never reads it.
    r.secrets
        .0
        .lock()
        .unwrap()
        .insert("llm.openrouter".into(), "sk-or-not-real".into());
    let v = r.dictate(300).await;
    let e = v.last.expect("an entry");
    assert_eq!(e.cleanup, CleanupMethod::Rules);
    assert_eq!(e.text, rule_cleanup(&e.raw, &[]));
    assert!(e.inserted);
    assert_eq!(
        e.latency.cleanup_ms,
        Some(0.0),
        "no wait: the rules are synchronous"
    );
    assert!(
        !r.phases().iter().any(|p| p == "cleaning"),
        "no cleaning phase between release and insert: {:?}",
        r.phases()
    );
    let ins = r.helper.sent_of("insert.text");
    assert_eq!(ins.len(), 1);
    assert_eq!(ins[0]["text"], e.text.as_str());
    assert!(!r.everything_said().contains("sk-or-not-real"));
    assert!(r.cloud.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn auto_cleanup_off_inserts_the_raw_text_with_only_the_dictionary() {
    let r = rig(RigOpts {
        config: old_cleanup_config(false),
        ..Default::default()
    });
    let e = r.dictate(300).await.last.expect("an entry");
    assert_eq!(e.cleanup, CleanupMethod::Off);
    assert_eq!(e.text, e.raw.trim());
}

#[test]
fn the_cleanup_is_the_rules_or_the_dictionary_only() {
    let dict = vec![DictionaryEntry {
        term: "qrtz".into(),
        aliases: vec!["cue arts".into()],
    }];
    let raw = "um so I I think cue arts ships friday";
    let on = cleanup_dictation(raw, &dict, true);
    assert_eq!(on.method, CleanupMethod::Rules);
    assert_eq!(on.text, rule_cleanup(raw, &dict));
    let off = cleanup_dictation(raw, &dict, false);
    assert_eq!(off.method, CleanupMethod::Off);
    assert_eq!(off.text, apply_dictionary(raw, &dict));
}

fn old_entry(id: &str, cleanup: &str, raw: &str, text: &str) -> Value {
    json!({"id": id, "at": 1760000000000u64, "raw": raw, "text": text, "cleanup": cleanup, "engine": "on-device",
           "failover": false, "insert": "ax-selected-text", "inserted": true, "durationMs": 1200,
           "latency": {"firstPartialMs": 180, "finalMs": 90, "cleanupMs": 640, "insertedMs": 760}})
}

fn old_talk_json() -> Vec<u8> {
    json!({
        "history": [
            old_entry("llm", "llm", "um so I I think we should ship", "I think we should ship."),
            old_entry("rules", "rules", "uh hello there", "Hello there."),
            old_entry("off", "off", "as said", "as said"),
        ],
        "dictionary": [{"term": "qrtz", "aliases": ["cue arts"]}],
    })
    .to_string()
    .into_bytes()
}

#[test]
fn an_old_talk_json_with_llm_cleaned_entries_loads_as_written() {
    let (st, dropped) = parse(&old_talk_json()).expect("a talk.json, not corrupt");
    assert_eq!(dropped, 0);
    let got: Vec<(&str, CleanupMethod, &str, &str, Option<f64>)> = st
        .history
        .iter()
        .map(|h| {
            (
                h.id.as_str(),
                h.cleanup,
                h.raw.as_str(),
                h.text.as_str(),
                h.latency.cleanup_ms,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "llm",
                CleanupMethod::Llm,
                "um so I I think we should ship",
                "I think we should ship.",
                Some(640.0)
            ),
            (
                "rules",
                CleanupMethod::Rules,
                "uh hello there",
                "Hello there.",
                Some(640.0)
            ),
            ("off", CleanupMethod::Off, "as said", "as said", Some(640.0)),
        ]
    );
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("talk.json");
    std::fs::write(&file, old_talk_json()).unwrap();
    let (st2, out) = load(&FsIo, &file, 1.0);
    assert_eq!(out, LoadOutcome::Loaded { dropped: 0 }, "never quarantined");
    assert_eq!(st2, st);
}

#[tokio::test(start_paused = true)]
async fn the_service_keeps_old_llm_entries_through_a_rewrite() {
    let r = rig(RigOpts {
        seed_store: Some(old_talk_json()),
        config: old_cleanup_config(true),
        ..Default::default()
    });
    assert_eq!(r.svc.history().len(), 3);
    r.dictate(300).await;
    let h = r.svc.history();
    assert_eq!(h.len(), 4);
    assert_eq!(h[0].cleanup, CleanupMethod::Rules, "the new one");
    let old = h.iter().find(|e| e.id == "llm").expect("kept");
    assert_eq!(old.cleanup, CleanupMethod::Llm);
    assert_eq!(old.text, "I think we should ship.");
    assert_eq!(old.raw, "um so I I think we should ship");
    let body: Value = serde_json::from_slice(&std::fs::read(&r.store).unwrap()).unwrap();
    let on_disk = body["history"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "llm")
        .unwrap()
        .clone();
    assert_eq!(on_disk["cleanup"], "llm", "{on_disk}");
    assert_eq!(on_disk["latency"]["cleanupMs"], 640, "{on_disk}");
    assert_eq!(body["dictionary"][0]["term"], "qrtz");
}

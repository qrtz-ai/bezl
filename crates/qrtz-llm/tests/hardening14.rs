//! Hardening cycle 14: the on-device summary on a huge transcript (the Tauri build's only summary until OpenRouter
//! lands, N53). The Q&A and the "can you" owner lookups scan the sentences per question, so a 3 h meeting is the case
//! to bound. Same algorithm as TS `extractiveSummary`; the outputs are pinned by spec/vectors/llm/extractive.json.
use std::time::Instant;

use qrtz_llm::extractive_summary;
use qrtz_model::Segment;
use serde_json::json;

fn seg(i: usize, speaker: &str, text: &str) -> Segment {
    serde_json::from_value(json!({
        "id": format!("s{i}"), "startMs": i * 1000, "endMs": i * 1000 + 900, "text": text,
        "speakerName": speaker, "source": "live"
    }))
    .unwrap()
}

/// 3 h at one line a second, two speakers, a question every fourth line, a request every tenth.
#[test]
fn three_hour_meeting_stays_bounded() {
    let segs: Vec<Segment> = (0..10_800)
        .map(|i| {
            let sp = if i % 2 == 0 { "Ana" } else { "Ben" };
            let text = match i % 10 {
                0 => "Can you send the rollout plan by Friday?",
                3 | 7 => "What is blocking the importer right now?",
                5 => "We need to fix the flaky upload test before the release.",
                _ => "The parser handles the new transcript format and the tests are green.",
            };
            seg(i, sp, text)
        })
        .collect();
    let t0 = Instant::now();
    let s = extractive_summary(&segs, None, &[], None);
    let ms = t0.elapsed().as_millis();
    eprintln!(
        "extractive 3 h, 10 800 lines: {ms} ms, {} action items, {} qa",
        s.action_items.len(),
        s.qa.len()
    );
    assert!(!s.summary.is_empty());
    assert!(ms < 10_000, "{ms} ms");
}

/// Worst case for the Q&A scan: one speaker asking only questions, so no answer is ever found and every question
/// scans every sentence.
#[test]
fn one_speaker_all_questions_stays_bounded() {
    let segs: Vec<Segment> = (0..10_800)
        .map(|i| seg(i, "Ana", "Is this thing on? Can anyone hear me?"))
        .collect();
    let t0 = Instant::now();
    let s = extractive_summary(&segs, None, &[], None);
    let ms = t0.elapsed().as_millis();
    eprintln!("extractive worst case, 21 600 questions: {ms} ms");
    assert!(s.qa.is_empty());
    assert!(ms < 30_000, "{ms} ms");
}

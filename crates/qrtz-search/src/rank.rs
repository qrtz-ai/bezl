//! The chat ranker (port of `queryTerms`, `rankSources` in chat.ts and `hybridRank` in embeddings.ts). Same
//! splitting as TS: lower-case, then NFKC, split on anything but letters, digits and combining marks (N47, ADR 0038);
//! a word with a Han, kana or Thai character becomes its bigrams, matched inside the line (N49, ADR 0047).
//! Pinned by the `ranks` of spec/stress/chat-cjk.json.
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use qrtz_storage::{bigrams, is_unsegmented, SegmentHit, CHAT_MAX_BIGRAMS};
use unicode_normalization::UnicodeNormalization;

/// One retrieved transcript line (`ChatSource`).
pub type ChatSource = SegmentHit;

const STOP: &str = "a an the and or but of to in on at for with is are was were be been it this that what who when where why how did do does i you we they he she me my our your about from as by can could should would will any all";

/// `.toLowerCase().normalize('NFKC').split(/[^\p{L}\p{N}\p{M}]+/u)` (empty strings at the edges kept, like JS).
fn split_words(s: &str) -> Vec<String> {
    static SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}\p{M}]+").unwrap());
    let n: String = s.to_lowercase().nfkc().collect();
    SEP.split(&n).map(str::to_string).collect()
}

/// `queryTerms`: distinct words of 2+ UTF-16 units that are not stop words, at most 12; then the plain ones, then
/// the distinct bigrams of the unsegmented ones (at most 16).
pub fn query_terms(q: &str) -> Vec<String> {
    let stop: HashSet<&str> = STOP.split(' ').collect();
    let mut seen = HashSet::new();
    let words: Vec<String> = split_words(q)
        .into_iter()
        .filter(|w| w.encode_utf16().count() > 1 && !stop.contains(w.as_str()))
        .filter(|w| seen.insert(w.clone()))
        .take(12)
        .collect();
    let mut out: Vec<String> = words.iter().filter(|w| !is_unsegmented(w)).cloned().collect();
    let mut gs = HashSet::new();
    let grams: Vec<String> = words
        .iter()
        .filter(|w| is_unsegmented(w))
        .flat_map(|w| bigrams(w))
        .filter(|g| gs.insert(g.clone()))
        .take(CHAT_MAX_BIGRAMS)
        .collect();
    out.extend(grams);
    out
}

/// `rankSources`: matched distinct terms (word prefix; a bigram anywhere in the line), weighted by rarity across the
/// candidates. Lines that match nothing are dropped. No terms: the first `k` as they are.
pub fn rank_sources(question: &str, sources: &[ChatSource], k: usize) -> Vec<ChatSource> {
    rank_indices(question, sources, k)
        .into_iter()
        .map(|i| sources[i].clone())
        .collect()
}

fn rank_indices(question: &str, sources: &[ChatSource], k: usize) -> Vec<usize> {
    let terms = query_terms(question);
    if terms.is_empty() {
        return (0..sources.len().min(k)).collect();
    }
    let words: Vec<Vec<String>> = sources
        .iter()
        .map(|s| split_words(&format!("{} {}", s.speaker.as_deref().unwrap_or(""), s.text)))
        .collect();
    let lines: Vec<String> = words.iter().map(|ws| ws.join(" ")).collect();
    let has = |i: usize, t: &str| {
        if is_unsegmented(t) {
            lines[i].contains(t)
        } else {
            words[i].iter().any(|w| w.starts_with(t))
        }
    };
    let n = sources.len() as f64;
    let df: Vec<f64> = terms
        .iter()
        .map(|t| (0..sources.len()).filter(|&i| has(i, t)).count() as f64)
        .collect();
    let mut scored: Vec<(usize, f64)> = (0..sources.len())
        .map(|i| {
            let mut score = 0.0;
            for (j, t) in terms.iter().enumerate() {
                if has(i, t) {
                    score += (1.0 + n / (1.0 + df[j])).ln();
                }
            }
            (i, score)
        })
        .filter(|x| x.1 > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
    scored.into_iter().take(k).map(|x| x.0).collect()
}

/// A candidate with its vector, if it has one (`VecSource`).
#[derive(Debug, Clone)]
pub struct VecSource {
    pub source: ChatSource,
    pub vec: Option<Vec<f32>>,
}

/// `hybridRank`: reciprocal-rank fusion (K = 60) of the lexical ranking and the cosine similarity to the question
/// (at least `min_sim`). Without a question vector it is the lexical ranking.
pub fn hybrid_rank(
    question: &str,
    candidates: &[VecSource],
    qvec: Option<&[f32]>,
    k: usize,
    min_sim: f64,
) -> Vec<ChatSource> {
    let plain: Vec<ChatSource> = candidates.iter().map(|c| c.source.clone()).collect();
    let Some(q) = qvec else {
        return rank_sources(question, &plain, k);
    };
    const K: f64 = 60.0;
    // Insertion order matters for ties (a JS Map iterates in insertion order; the sort is stable).
    let mut order: Vec<usize> = Vec::new();
    let mut score: HashMap<usize, f64> = HashMap::new();
    let mut add = |c: usize, s: f64| {
        let e = score.entry(c).or_insert_with(|| {
            order.push(c);
            0.0
        });
        *e += s;
    };
    for (i, c) in rank_indices(question, &plain, plain.len())
        .into_iter()
        .enumerate()
    {
        add(c, 1.0 / (K + i as f64));
    }
    let mut sims: Vec<(usize, f64)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.vec.as_ref().map(|v| (i, crate::embedder::dot(q, v))))
        .filter(|x| x.1 >= min_sim)
        .collect();
    sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    for (i, (c, _)) in sims.into_iter().enumerate() {
        add(c, 1.0 / (K + i as f64));
    }
    let mut all: Vec<(usize, f64)> = order.iter().map(|c| (*c, score[c])).collect();
    all.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    all.into_iter().take(k).map(|(c, _)| plain[c].clone()).collect()
}

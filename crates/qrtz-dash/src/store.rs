//! One dashboard's state and the agent API (spec "Server design"). In memory for now (ADR 0228: storage is stubbed;
//! the Durable Object SQLite tables map one to one onto the maps below).
use crate::blocks::{validate_append, validate_put, BlockType};
use crate::crypto::{ct_eq, random_hex, Pepper};
use crate::*;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Widget,
    Agent,
    Dashboard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum AgentStatus {
    Pending,
    Approved,
    Rejected,
}

struct Agent {
    id: String,
    name: String,
    skeleton: String,
    description: String,
    fingerprint: String,
    token_hash: String,
    status: AgentStatus,
    /// What the last heartbeat said (`running` / `idle`), if any.
    heartbeat: Option<String>,
    last_seen: u64,
    created_at: u64,
    flagged: bool,
}

struct Widget {
    id: String,
    key: String,
    ty: BlockType,
    title: String,
    state: Option<Value>,
    updated_at: u64,
    events: VecDeque<(u64, Value)>,
}

struct Code {
    hash: String,
    expires_at: u64,
    used: bool,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn new(method: &str, path: &str) -> Self {
        Request {
            method: method.into(),
            path: path.into(),
            headers: vec![],
            body: vec![],
        }
    }
    pub fn bearer(mut self, token: &str) -> Self {
        self.headers
            .push(("Authorization".into(), format!("Bearer {token}")));
        self
    }
    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn json(mut self, v: Value) -> Self {
        self.body = serde_json::to_vec(&v).unwrap_or_default();
        self
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: Value,
}

fn resp(status: u16, mut body: Value) -> Response {
    body["readme_version"] = json!(README_VERSION);
    Response { status, body }
}
fn fail(status: u16, error: &str) -> Response {
    resp(status, json!({ "error": error }))
}
fn stop(status: u16, error: &str) -> Response {
    resp(status, json!({ "error": error, "action": "stop" }))
}
fn gone(scope: Scope) -> Response {
    resp(
        410,
        json!({ "error": "deleted", "action": "stop", "scope": scope }),
    )
}

/// Names that would pass for qrtz itself.
const RESERVED: &[&str] = &[
    "security",
    "system",
    "admin",
    "agentdash",
    "qrtz",
    "dash",
    "support",
    "you",
];

/// Strip format (Cf) and bidi controls and other controls, collapse spaces.
fn clean_name(s: &str) -> String {
    let bad = |c: char| {
        c.is_control()
            || matches!(c as u32, 0x00AD | 0x061C | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB)
    };
    s.chars()
        .filter(|c| !bad(*c))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A crude confusable skeleton (ADR 0228 stub for UTS #39): lowercase ascii letters/digits with the common
/// look-alikes folded.
fn skeleton(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            '0' | 'о' | 'ο' => Some('o'),
            '1' | 'l' | 'і' | 'ı' | '|' => Some('i'),
            'а' | 'α' => Some('a'),
            'е' | 'ε' => Some('e'),
            'с' => Some('c'),
            'р' | 'ρ' => Some('p'),
            'х' => Some('x'),
            'у' => Some('y'),
            'ѕ' => Some('s'),
            '5' => Some('s'),
            c if c.is_ascii_alphanumeric() => Some(c),
            _ => None,
        })
        .collect::<String>()
        .replace("rn", "m")
}

fn valid_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && !k.starts_with('-')
        && !k.ends_with('-')
        && !k.contains("--")
        && k.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn luhn(d: &[u8]) -> bool {
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &x)| {
            let x = u32::from(x);
            if !i.is_multiple_of(2) {
                let y = x * 2;
                if y > 9 {
                    y - 9
                } else {
                    y
                }
            } else {
                x
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Token prefixes of common keys and tokens (lowercased). A word with one of these and at least 12 more characters
/// is redacted.
const SECRET_PREFIXES: &[&str] = &[
    "sk-ant-",
    "sk-",
    "sk_live_",
    "rk_live_",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "ghr_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-",
    "xoxr-",
    "xapp-",
    "akia",
    "asia",
    "at_",
    "aiza",
    "glpat-",
    "hf_",
    "npm_",
];

/// Characters a secret can't contain, so they split words (Bozniak M1): whitespace plus `= : " ' , ; ( ) [ ] { } < >`.
fn is_split(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '=' | ':' | '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
        )
}

/// Shannon entropy in bits per character.
fn entropy(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    for b in s.bytes() {
        counts[usize::from(b)] += 1;
    }
    let n = s.len() as f64;
    counts
        .iter()
        .filter(|c| **c > 0)
        .map(|c| {
            let p = f64::from(*c) / n;
            -p * p.log2()
        })
        .sum()
}

fn is_b64url(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_')
}

/// `eyJ…` header, three base64url segments.
fn is_jwt(w: &str) -> bool {
    let parts: Vec<&str> = w.split('.').collect();
    parts.len() == 3
        && w.starts_with("eyJ")
        && parts[0].len() >= 10
        && parts[1].len() >= 10
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(is_b64url))
}

/// Zero-width characters dropped before the prefix check (Bozniak L): U+200B–U+200D, U+2060, U+FEFF.
fn is_zero_width(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}')
}

/// A key prefix at the start of the word or after any byte that isn't an ASCII letter or digit, so `x|ghp_…`,
/// `a&sk-ant-…` and `@xoxb-…` are caught (Bozniak M). The token (letters, digits, `-`, `_` from the prefix on)
/// must run 12+ characters past the prefix. The word is checked with zero-width characters removed and NFKC
/// applied; bytes are compared, never sliced, so non-ASCII text can't panic (Bozniak H1).
fn has_secret_prefix(w: &str) -> bool {
    let norm: String = w.chars().filter(|c| !is_zero_width(*c)).nfkc().collect();
    let b = norm.to_ascii_lowercase().into_bytes();
    (0..b.len())
        .filter(|&i| i == 0 || !b[i - 1].is_ascii_alphanumeric())
        .any(|i| {
            let run = b[i..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
                .count();
            let tok = &b[i..i + run];
            SECRET_PREFIXES
                .iter()
                .any(|p| tok.starts_with(p.as_bytes()) && tok.len() >= p.len() + 12)
        })
}

/// Identifier-like: the camelCase / ALLCAPS / digit runs (`[A-Z]?[a-z]+|[A-Z]+(?![a-z])|[0-9]+`) average 4+
/// characters. `_`, `-` and other characters separate runs (Bozniak H2).
fn is_wordy(w: &str) -> bool {
    let b = w.as_bytes();
    let (mut i, mut count, mut total) = (0, 0usize, 0usize);
    while i < b.len() {
        let c = b[i];
        let len = if c.is_ascii_uppercase() && b.get(i + 1).is_some_and(u8::is_ascii_lowercase) {
            1 + b[i + 1..].iter().take_while(|c| c.is_ascii_lowercase()).count()
        } else if c.is_ascii_lowercase() {
            b[i..].iter().take_while(|c| c.is_ascii_lowercase()).count()
        } else if c.is_ascii_uppercase() {
            let k = b[i..].iter().take_while(|c| c.is_ascii_uppercase()).count();
            // the last capital starts a camelCase run when lowercase follows
            if b.get(i + k).is_some_and(u8::is_ascii_lowercase) {
                k - 1
            } else {
                k
            }
        } else if c.is_ascii_digit() {
            b[i..].iter().take_while(|c| c.is_ascii_digit()).count()
        } else {
            i += 1;
            continue;
        };
        count += 1;
        total += len;
        i += len;
    }
    count > 0 && total as f64 / count as f64 >= 4.0
}

/// An identifier: wordy as a whole, or split on `_` / `-` with every segment an identifier segment, so
/// `ENROLL_V6_48_FAILS_PER_MIN` stays (Bozniak L, review of 8fb9e555).
fn is_identifier(w: &str) -> bool {
    is_wordy(w) || (w.contains(['_', '-']) && w.split(['_', '-']).all(is_ident_segment))
}

/// One `_` / `-` segment of an identifier: wordy, 3 characters or fewer, all digits, or one case class of at
/// most 7 characters (`[A-Z0-9]{1,7}` or `[a-z]{1,7}`), so random keys of short mixed-case segments are not
/// identifiers (Bozniak H, review of ded56dd5).
fn is_ident_segment(p: &str) -> bool {
    let b = p.as_bytes();
    is_wordy(p)
        || b.len() <= 3
        || b.iter().all(u8::is_ascii_digit)
        || (b.len() <= 7
            && (b.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                || b.iter().all(u8::is_ascii_lowercase)))
}

/// The entropy and JWT checks on one candidate, trimmed like a word. A word with no `/` that has `_` or `-` is
/// also entropy-checked with those separators removed, so a key split into short segments is still dense
/// (Bozniak, review of ce60c75f).
fn is_dense(s: &str) -> bool {
    let c = s.trim_matches(|c: char| matches!(c, '.' | '!' | '?' | '`' | '*'));
    is_high_entropy(c) || is_jwt(c) || is_dense_joined(c)
}

/// `c` without `_` / `-`, entropy-checked when `c` has no `/`, had a separator and is still 24+ characters.
fn is_dense_joined(c: &str) -> bool {
    if c.contains('/') || !c.contains(['_', '-']) {
        return false;
    }
    let j: String = c.chars().filter(|&x| x != '_' && x != '-').collect();
    j.len() >= 24 && is_high_entropy(&j)
}

/// The high-entropy rule (Bozniak H2, his prototype `rule`): 24+ base64 / base64url characters mixing 2+ of
/// lowercase, uppercase and digits, not all hex (the hex rule decides those), not an identifier, not a path of
/// identifiers or short segments, and at least 0.8·log2(min(len, 64)) bits per character.
fn is_high_entropy(core: &str) -> bool {
    let w = core.trim_end_matches('=');
    if w.len() < 24
        || !w
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'_' | b'-'))
    {
        return false;
    }
    let classes = [
        w.bytes().any(|c| c.is_ascii_lowercase()),
        w.bytes().any(|c| c.is_ascii_uppercase()),
        w.bytes().any(|c| c.is_ascii_digit()),
    ];
    if classes.iter().filter(|x| **x).count() < 2 || w.bytes().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let path_ok = w.contains('/')
        && w.split('/').all(|p| {
            p.len() <= 24
                && p.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
                && (is_wordy(p) || p.len() < 8)
        });
    if path_ok || is_identifier(w) {
        return false;
    }
    entropy(w) >= 0.8 * (w.len().min(64) as f64).log2()
}

/// One word (between split characters) that looks like a secret. `sha_ctx`: the word follows `sha256:` or
/// `sha256=`, so a 64-hex digest is not a secret.
fn is_secret_word(w: &str, sha_ctx: bool) -> bool {
    let core = w.trim_matches(|c: char| matches!(c, '.' | '!' | '?' | '`' | '*'));
    let keyish = has_secret_prefix(w);
    let b = core.as_bytes();
    let ssn = b.len() == 11
        && b[3] == b'-'
        && b[6] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 3 || i == 6 || c.is_ascii_digit());
    // Exactly 40 hex is a git object hash; 64 hex after `sha256:` / `sha256=` is a digest (Bozniak L3, H2).
    let long_hex = core.len() >= 32
        && core.len() != 40
        && !(core.len() == 64 && sha_ctx)
        && core.bytes().all(|c| c.is_ascii_hexdigit());
    // One non-ASCII character can't hide a secret (Bozniak M, review of 8fb9e555): the entropy and JWT checks also
    // run on the NFKC form without zero-width characters, that form with non-ASCII removed, and every ASCII run
    // of 24+ characters in the word or its NFKC form.
    let dense = || {
        let norm: String = w.chars().filter(|c| !is_zero_width(*c)).nfkc().collect();
        let ascii: String = norm.chars().filter(char::is_ascii).collect();
        is_dense(core)
            || is_dense(&norm)
            || is_dense(&ascii)
            || [w, norm.as_str()]
                .iter()
                .flat_map(|x| x.split(|c: char| !c.is_ascii()))
                .any(|r| r.len() >= 24 && is_dense(r))
    };
    keyish || ssn || long_hex || dense()
}

/// PEM blocks (`-----BEGIN … -----` through the matching `-----END … -----`, or to the end) are redacted whole.
fn scrub_pem(s: &mut String) -> bool {
    let mut hit = false;
    // Case-insensitive (Bozniak L2); ASCII lowercasing keeps byte offsets.
    while let Some(start) = s.to_ascii_lowercase().find("-----begin") {
        let rest = s[start..].to_ascii_lowercase();
        let end = rest
            .find("-----end")
            .map(|e| {
                let after = &rest[e + 8..];
                // the END line's closing dashes
                after.find("-----").map_or(rest.len(), |d| e + 8 + d + 5)
            })
            .unwrap_or(rest.len());
        s.replace_range(start..start + end, "[redacted]");
        hit = true;
    }
    hit
}

/// The secret scanner: redact PEM blocks, key/token shapes, JWTs, high-entropy strings, SSNs and card numbers.
/// Returns true if anything was hit.
fn scrub(s: &mut String) -> bool {
    let mut hit = scrub_pem(s);
    let mut out = String::with_capacity(s.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String, hit: &mut bool| {
        if !word.is_empty() {
            let tail = out.len().checked_sub(7).and_then(|i| out.get(i..));
            let sha_ctx =
                tail.is_some_and(|t| t.eq_ignore_ascii_case("sha256:") || t.eq_ignore_ascii_case("sha256="));
            if is_secret_word(word, sha_ctx) {
                out.push_str("[redacted]");
                *hit = true;
            } else {
                out.push_str(word);
            }
            word.clear();
        }
    };
    for c in s.chars() {
        if is_split(c) {
            flush(&mut word, &mut out, &mut hit);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out, &mut hit);
    *s = out;
    // Card numbers: runs of digits with optional spaces/dashes, 13-19 digits, Luhn-valid.
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let mut j = i;
            let mut digits = vec![];
            while j < chars.len()
                && (chars[j].is_ascii_digit()
                    || ((chars[j] == ' ' || chars[j] == '-')
                        && chars.get(j + 1).is_some_and(char::is_ascii_digit)))
            {
                if let Some(d) = chars[j].to_digit(10) {
                    digits.push(d as u8);
                }
                j += 1;
            }
            if (13..=19).contains(&digits.len()) && luhn(&digits) {
                out.push_str("[redacted]");
                hit = true;
            } else {
                out.extend(&chars[i..j]);
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    *s = out;
    hit
}

/// Scrub every string in a payload, object keys included (Bozniak M1).
fn scrub_value(v: &mut Value) -> bool {
    match v {
        Value::String(s) => scrub(s),
        Value::Array(a) => a.iter_mut().fold(false, |h, x| scrub_value(x) | h),
        Value::Object(o) => {
            let mut hit = false;
            let old = std::mem::take(o);
            for (mut k, mut x) in old {
                hit |= scrub(&mut k);
                hit |= scrub_value(&mut x);
                o.insert(k, x);
            }
            hit
        }
        _ => false,
    }
}

const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";

/// `n` characters from [`CODE_ALPHABET`], uniform: rejection sampling (bytes >= 240 = 8 x 30 are redrawn), never
/// `% 30`, which favours the first 16 characters (Bozniak M2).
fn code_chars(n: usize) -> String {
    let limit = (256 / CODE_ALPHABET.len() * CODE_ALPHABET.len()) as u8;
    let mut out = String::with_capacity(n);
    while out.len() < n {
        let mut b = [0u8; 16];
        crate::crypto::os_random(&mut b);
        for x in b {
            if x < limit && out.len() < n {
                out.push(char::from(CODE_ALPHABET[usize::from(x) % CODE_ALPHABET.len()]));
            }
        }
    }
    out
}

/// One dashboard.
pub struct Dash {
    pepper: Pepper,
    deleted: bool,
    codes: Vec<Code>,
    /// Failed enrollments in the last minute (dashboard-wide), and since the last rotation.
    enroll_failures: VecDeque<u64>,
    enroll_failures_total: usize,
    agents: Vec<Agent>,
    /// agent id → key → widget.
    widgets: HashMap<String, BTreeMap<String, Widget>>,
    tombstones: HashSet<(Scope, String)>,
    idempotency: HashMap<(String, String), (u64, Response)>,
    agent_writes: HashMap<String, VecDeque<u64>>,
    dash_writes: VecDeque<u64>,
    audit: Vec<Value>,
    next_id: u64,
}

impl Dash {
    pub fn new(pepper: Pepper) -> Self {
        Dash {
            pepper,
            deleted: false,
            codes: vec![],
            agents: vec![],
            widgets: HashMap::new(),
            tombstones: HashSet::new(),
            idempotency: HashMap::new(),
            agent_writes: HashMap::new(),
            dash_writes: VecDeque::new(),
            audit: vec![],
            next_id: 1,
            enroll_failures: VecDeque::new(),
            enroll_failures_total: 0,
        }
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}_{}{}", self.next_id, &random_hex(3))
    }

    fn log(&mut self, event: &str, agent: Option<&str>, now: u64) {
        self.audit
            .push(json!({"event": event, "agentId": agent, "at": now}));
    }

    // ---- the user's side (dash:* IPC) ----

    /// Issue a single-use enrollment code (`ENR-XXXX-XXXX`, 15 min). Only its hash is kept.
    pub fn issue_code(&mut self, now: u64) -> Option<String> {
        if self.deleted {
            return None;
        }
        let c = code_chars(8);
        let code = format!("ENR-{}-{}", &c[..4], &c[4..]);
        self.codes.push(Code {
            hash: self.pepper.hash(&code),
            expires_at: now + ENROLL_TTL_MS,
            used: false,
        });
        self.log("code-issued", None, now);
        Some(code)
    }

    /// Rotate the enrollment secret: every outstanding code stops working; agent tokens are untouched.
    pub fn rotate_codes(&mut self, now: u64) {
        self.codes.clear();
        self.enroll_failures.clear();
        self.enroll_failures_total = 0;
        self.log("codes-rotated", None, now);
    }

    fn agent_mut(&mut self, id: &str) -> Option<&mut Agent> {
        self.agents.iter_mut().find(|a| a.id == id)
    }

    /// Approve a pending agent (an account-security gate, not a channel: the agent only sees writes accepted).
    pub fn approve(&mut self, agent_id: &str, now: u64) -> bool {
        let ok = matches!(self.agent_mut(agent_id), Some(a) if a.status == AgentStatus::Pending);
        if ok {
            let a = self.agent_mut(agent_id).expect("checked");
            a.status = AgentStatus::Approved;
            self.log("approved", Some(agent_id), now);
        }
        ok
    }

    /// Reject a pending agent: its token answers 401 stop.
    pub fn reject(&mut self, agent_id: &str, now: u64) -> bool {
        let ok = matches!(self.agent_mut(agent_id), Some(a) if a.status == AgentStatus::Pending);
        if ok {
            self.agent_mut(agent_id).expect("checked").status = AgentStatus::Rejected;
            self.log("rejected", Some(agent_id), now);
        }
        ok
    }

    /// Delete one widget: tombstoned, so that agent's writes to (and re-registration of) the key answer 410.
    pub fn delete_widget(&mut self, agent_id: &str, key: &str, now: u64) -> bool {
        let removed = self
            .widgets
            .get_mut(agent_id)
            .and_then(|w| w.remove(key))
            .is_some();
        if removed {
            self.tombstones
                .insert((Scope::Widget, format!("{agent_id}/{key}")));
            self.log("widget-deleted", Some(agent_id), now);
        }
        removed
    }

    /// Delete an agent and its widgets: its token answers 410 scope agent from now on.
    pub fn delete_agent(&mut self, agent_id: &str, now: u64) -> bool {
        let Some(i) = self.agents.iter().position(|a| a.id == agent_id) else {
            return false;
        };
        let a = self.agents.remove(i);
        self.widgets.remove(agent_id);
        self.tombstones.insert((Scope::Agent, agent_id.to_owned()));
        self.tombstones
            .insert((Scope::Agent, format!("token:{}", a.token_hash)));
        self.log("agent-deleted", Some(agent_id), now);
        true
    }

    /// Delete the whole dashboard: everything answers 410 scope dashboard; no new codes.
    pub fn delete_dashboard(&mut self, now: u64) {
        self.deleted = true;
        self.codes.clear();
        self.widgets.clear();
        self.tombstones.insert((Scope::Dashboard, String::new()));
        self.log("dashboard-deleted", None, now);
    }

    /// Drop history older than [`RETENTION_DAYS`], expired codes and idempotency records, and old rate windows.
    pub fn prune(&mut self, now: u64) {
        let cutoff = now.saturating_sub(RETENTION_DAYS * DAY_MS);
        for ws in self.widgets.values_mut() {
            for w in ws.values_mut() {
                while w.events.front().is_some_and(|(t, _)| *t < cutoff) {
                    w.events.pop_front();
                }
            }
        }
        self.audit.retain(|a| a["at"].as_u64().unwrap_or(0) >= cutoff);
        self.codes.retain(|c| !c.used && c.expires_at > now);
        self.idempotency.retain(|_, (exp, _)| *exp > now);
    }

    /// The read-only view for the user's UI. Holds no token, hash or code.
    pub fn view(&mut self, now: u64) -> Value {
        self.prune(now);
        let agents: Vec<Value> = self
            .agents
            .iter()
            .filter(|a| a.status != AgentStatus::Rejected)
            .map(|a| {
                let widgets: Vec<Value> = self
                    .widgets
                    .get(&a.id)
                    .into_iter()
                    .flat_map(|w| w.values())
                    .map(|w| {
                        json!({
                            "key": w.key, "type": w.ty, "title": w.title, "state": w.state,
                            "events": w.events.iter().rev().take(50).rev().map(|(t, e)| json!({"at": t, "data": e})).collect::<Vec<_>>(),
                            "updatedAt": w.updated_at,
                        })
                    })
                    .collect();
                json!({
                    "id": a.id, "name": a.name, "description": a.description, "fingerprint": a.fingerprint,
                    "status": a.status, "heartbeat": a.heartbeat, "lastSeen": a.last_seen,
                    "stale": a.status == AgentStatus::Approved && now.saturating_sub(a.last_seen) > STALE_AFTER_MS,
                    "flagged": a.flagged, "createdAt": a.created_at, "widgets": widgets,
                })
            })
            .collect();
        json!({
            "origin": DASH_ORIGIN, "readmeVersion": README_VERSION, "deleted": self.deleted,
            "retentionDays": RETENTION_DAYS, "agents": agents,
            "audit": self.audit.iter().rev().take(50).cloned().collect::<Vec<_>>(),
        })
    }

    // ---- the agent's side (HTTP) ----

    /// Handle one agent request. Never reads anything back to the agent but its own ids and the outcome.
    pub fn handle(&mut self, req: &Request, now: u64) -> Response {
        self.prune(now);
        // Tokens never go in URLs, and nothing is selected by query string.
        let path = req.path.split(['?', '#']).next().unwrap_or("");
        if req.method == "GET" && path == "/v1/readme" {
            return resp(200, json!({ "url": format!("{DASH_ORIGIN}/v1/readme") }));
        }
        let max = if path == "/v1/batch" {
            MAX_BATCH_BODY_BYTES
        } else {
            MAX_BODY_BYTES
        };
        let declared = req
            .get("Content-Length")
            .and_then(|v| v.trim().parse::<usize>().ok());
        if req.body.len() > max || declared.is_some_and(|d| d > max) {
            return fail(413, "body too large");
        }
        if self.deleted {
            return gone(Scope::Dashboard);
        }
        let body: Value = if req.body.is_empty() {
            Value::Null
        } else {
            match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(_) => return fail(422, "body is not json"),
            }
        };
        if req.method == "POST" && path == "/v1/enroll" {
            return self.enroll(&body, now);
        }
        // Auth: bearer agent_token → agent. Codes and anything else are never write bearers.
        let token = req
            .get("Authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim);
        let Some(token) = token.filter(|t| t.starts_with("at_")) else {
            return stop(401, "missing or invalid token");
        };
        let hash = self.pepper.hash(token);
        let Some(idx) = self.agents.iter().position(|a| ct_eq(&a.token_hash, &hash)) else {
            // A deleted agent's token: its hash stays in the tombstone set so it answers 410, not 401.
            if self.tombstones.contains(&(Scope::Agent, format!("token:{hash}"))) {
                return gone(Scope::Agent);
            }
            return stop(401, "token invalid or revoked");
        };
        match self.agents[idx].status {
            AgentStatus::Rejected => return stop(401, "token invalid or revoked"),
            AgentStatus::Pending => return resp(403, json!({"error": "pending"})),
            AgentStatus::Approved => {}
        }
        let agent_id = self.agents[idx].id.clone();
        let idem = req.get("Idempotency-Key").map(str::to_owned).or_else(|| {
            body.get("seq")
                .and_then(Value::as_u64)
                .map(|s| format!("seq:{s}"))
        });
        if let Some(k) = &idem {
            if let Some((_, r)) = self.idempotency.get(&(agent_id.clone(), k.clone())) {
                return r.clone();
            }
        }
        let ops = if path == "/v1/batch" {
            body["ops"].as_array().map_or(1, Vec::len).max(1)
        } else {
            1
        };
        if let Some(r) = self.rate(&agent_id, ops, now) {
            return r;
        }
        self.agents[idx].last_seen = now;
        let r = self.route(&agent_id, &req.method, path, body, now);
        if let Some(k) = idem {
            if r.status < 300 {
                self.idempotency
                    .insert((agent_id, k), (now + IDEMPOTENCY_TTL_MS, r.clone()));
            }
        }
        r
    }

    fn rate(&mut self, agent_id: &str, n: usize, now: u64) -> Option<Response> {
        let win = now.saturating_sub(60_000);
        let a = self.agent_writes.entry(agent_id.to_owned()).or_default();
        while a.front().is_some_and(|t| *t <= win) {
            a.pop_front();
        }
        while self.dash_writes.front().is_some_and(|t| *t <= win) {
            self.dash_writes.pop_front();
        }
        if a.len() + n > WRITES_PER_MIN_AGENT || self.dash_writes.len() + n > WRITES_PER_MIN_DASHBOARD {
            let oldest = a
                .front()
                .copied()
                .unwrap_or(now)
                .min(self.dash_writes.front().copied().unwrap_or(now));
            return Some(resp(
                429,
                json!({"error": "rate limited", "retry_after_ms": (oldest + 60_000).saturating_sub(now).max(1000)}),
            ));
        }
        for _ in 0..n {
            a.push_back(now);
            self.dash_writes.push_back(now);
        }
        None
    }

    fn enroll(&mut self, body: &Value, now: u64) -> Response {
        // Bozniak M2: wrong codes are rate limited dashboard-wide (not per agent: there is no agent yet).
        let win = now.saturating_sub(60_000);
        while self.enroll_failures.front().is_some_and(|t| *t <= win) {
            self.enroll_failures.pop_front();
        }
        if self.enroll_failures.len() >= ENROLL_FAILS_PER_MIN {
            let oldest = self.enroll_failures.front().copied().unwrap_or(now);
            return resp(
                429,
                json!({"error": "too many wrong codes", "retry_after_ms": (oldest + 60_000).saturating_sub(now).max(1000)}),
            );
        }
        let code = body["code"].as_str().unwrap_or("").trim().to_ascii_uppercase();
        let hash = self.pepper.hash(&code);
        let Some(c) = self
            .codes
            .iter_mut()
            .find(|c| ct_eq(&c.hash, &hash) && !c.used && c.expires_at > now)
        else {
            self.enroll_failures.push_back(now);
            self.enroll_failures_total += 1;
            if self.enroll_failures_total >= ENROLL_FAILS_BEFORE_ROTATION {
                // Forced rotation: every outstanding code dies; the user issues a new one.
                self.rotate_codes(now);
                self.log("codes-rotated-after-failures", None, now);
            }
            return fail(401, "enrollment code is used, expired or wrong");
        };
        c.used = true;
        let name = clean_name(body["name"].as_str().unwrap_or(""));
        let mut description = clean_name(body["description"].as_str().unwrap_or(""));
        if name.is_empty() || name.chars().count() > 40 || description.chars().count() > 200 {
            return fail(422, "name is 1-40 characters, description at most 200");
        }
        let sk = skeleton(&name);
        if sk.is_empty() || RESERVED.iter().any(|r| skeleton(r) == sk) {
            return fail(422, "that name is reserved");
        }
        if self
            .agents
            .iter()
            .any(|a| a.skeleton == sk && a.status != AgentStatus::Rejected)
        {
            return fail(409, "an agent with that name already exists");
        }
        if self.agents.len() >= MAX_AGENTS {
            return fail(422, "this dashboard has the most agents it can hold");
        }
        let flagged = scrub(&mut description);
        let token = format!("at_{}", random_hex(32));
        let token_hash = self.pepper.hash(&token);
        let f = token_hash[..8].to_ascii_uppercase();
        let fingerprint = format!("{}-{}", &f[..4], &f[4..]);
        let id = self.id("ag");
        self.agents.push(Agent {
            id: id.clone(),
            name,
            skeleton: sk,
            description,
            fingerprint: fingerprint.clone(),
            token_hash,
            status: AgentStatus::Pending,
            heartbeat: None,
            last_seen: now,
            created_at: now,
            flagged,
        });
        self.log("enrolled", Some(&id), now);
        resp(
            201,
            json!({"agent_id": id, "agent_token": token, "status": "pending", "fingerprint": fingerprint}),
        )
    }

    fn route(&mut self, agent: &str, method: &str, path: &str, body: Value, now: u64) -> Response {
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        match (method, parts.as_slice()) {
            ("POST", ["v1", "heartbeat"]) => {
                let s = body["status"].as_str();
                if !matches!(s, Some("running" | "idle")) {
                    return fail(422, "status is running or idle");
                }
                let a = self.agent_mut(agent).expect("authed");
                a.heartbeat = s.map(str::to_owned);
                resp(200, json!({"ok": true}))
            }
            ("POST", ["v1", "widgets"]) => self.register(agent, &body, now),
            ("PUT", ["v1", "widgets", key]) => self.write(agent, key, false, body, now),
            ("POST", ["v1", "widgets", key, "events"]) => self.write(agent, key, true, body, now),
            ("POST", ["v1", "batch"]) => {
                let Some(ops) = body["ops"].as_array() else {
                    return fail(422, "ops is a list");
                };
                if ops.is_empty() || ops.len() > BATCH_MAX_OPS {
                    return fail(422, &format!("a batch has 1 to {BATCH_MAX_OPS} ops"));
                }
                let results: Vec<Value> = ops
                    .clone()
                    .into_iter()
                    .map(|op| {
                        let key = op["key"].as_str().unwrap_or("").to_owned();
                        let r = match op["op"].as_str() {
                            Some("put") => self.write(agent, &key, false, op["data"].clone(), now),
                            Some("append") => self.write(agent, &key, true, op["data"].clone(), now),
                            _ => fail(422, "op is put or append"),
                        };
                        let mut b = r.body;
                        b.as_object_mut().map(|o| o.remove("readme_version"));
                        json!({"key": key, "status": r.status, "result": b})
                    })
                    .collect();
                resp(200, json!({ "results": results }))
            }
            _ => fail(404, "no such endpoint"),
        }
    }

    fn register(&mut self, agent: &str, body: &Value, now: u64) -> Response {
        let key = body["key"].as_str().unwrap_or("");
        if !valid_key(key) {
            return fail(422, "key is lowercase kebab-case");
        }
        if self
            .tombstones
            .contains(&(Scope::Widget, format!("{agent}/{key}")))
        {
            return gone(Scope::Widget);
        }
        let Some(ty) = body["type"].as_str().and_then(BlockType::parse) else {
            return fail(422, "unknown type");
        };
        let mut title = clean_name(body["title"].as_str().unwrap_or(key));
        if title.chars().count() > 80 {
            return fail(422, "title is at most 80 characters");
        }
        let flagged = scrub(&mut title);
        let ws = self.widgets.entry(agent.to_owned()).or_default();
        if let Some(w) = ws.get(key) {
            if w.ty != ty {
                return fail(409, "key exists with another type");
            }
            return resp(201, json!({"key": key, "widget_id": w.id}));
        }
        if ws.len() >= MAX_WIDGETS_PER_AGENT {
            return fail(422, "at most 20 widgets per agent");
        }
        let id = self.id("w");
        self.widgets.entry(agent.to_owned()).or_default().insert(
            key.to_owned(),
            Widget {
                id: id.clone(),
                key: key.to_owned(),
                ty,
                title,
                state: None,
                updated_at: now,
                events: VecDeque::new(),
            },
        );
        if flagged {
            self.agent_mut(agent).expect("authed").flagged = true;
        }
        resp(201, json!({"key": key, "widget_id": id}))
    }

    fn write(&mut self, agent: &str, key: &str, append: bool, mut data: Value, now: u64) -> Response {
        if self
            .tombstones
            .contains(&(Scope::Widget, format!("{agent}/{key}")))
        {
            return gone(Scope::Widget);
        }
        let Some(w) = self.widgets.get(agent).and_then(|ws| ws.get(key)) else {
            return resp(
                404,
                json!({"error": "no such widget", "action": "register_own_widget"}),
            );
        };
        let ty = w.ty;
        if let Some(o) = data.as_object_mut() {
            o.remove("seq");
        }
        let checked = if append {
            validate_append(ty, &data)
        } else {
            validate_put(ty, &data)
        };
        if let Err(e) = checked {
            return fail(422, &e.0);
        }
        let flagged = scrub_value(&mut data);
        let w = self
            .widgets
            .get_mut(agent)
            .and_then(|ws| ws.get_mut(key))
            .expect("checked");
        w.updated_at = now;
        if append {
            let cap = if ty == BlockType::Log {
                LOG_RING
            } else {
                CHART_RING
            };
            w.events.push_back((now, data));
            while w.events.len() > cap {
                w.events.pop_front();
            }
        } else {
            w.state = Some(data);
        }
        if flagged {
            self.agent_mut(agent).expect("authed").flagged = true;
        }
        resp(200, json!({"ok": true}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scanner_redacts_keys_cards_and_ssns() {
        for (inp, out) in [
            ("key sk-abcdefghijklmnop1234 ok", "key [redacted] ok"),
            ("card 4111 1111 1111 1111 x", "card [redacted] x"),
            ("ssn 123-45-6789", "ssn [redacted]"),
            ("pr 1234 of 5678", "pr 1234 of 5678"),
        ] {
            let mut s = inp.to_owned();
            assert_eq!(scrub(&mut s), inp != out);
            assert_eq!(s, out);
        }
    }

    fn red(inp: &str) -> (bool, String) {
        let mut s = inp.to_owned();
        (scrub(&mut s), s)
    }

    #[test]
    fn scrub_splits_on_assignment_and_quote_punctuation() {
        for inp in [
            "token=ghp_abcdefghijklmnopqrstuvwx",
            "key:sk-abcdefghijklmnop1234",
            "\"xoxb-123456789012-abcdef\"",
            "('AKIAABCDEFGHIJKLMNOP')",
            "[npm_abcdefghijklmnopqrst]",
            "{hf_abcdefghijklmnopqrstu}",
            "<ghu_abcdefghijklmnopqrstuv>",
            "a;ghr_abcdefghijklmnopqrstuv,b",
        ] {
            let (hit, out) = red(inp);
            assert!(
                hit && out.contains("[redacted]") && !out.contains("abcdefghijkl"),
                "{inp} -> {out}"
            );
        }
    }

    #[test]
    fn scrub_knows_the_new_prefixes() {
        for p in [
            "asia", "xoxs-", "xoxr-", "xapp-", "ghu_", "ghr_", "sk-ant-", "hf_", "npm_",
        ] {
            let (hit, out) = red(&format!("x {p}ABCDEFGH12345678 y"));
            assert!(hit, "{p}");
            assert_eq!(out, "x [redacted] y", "{p}");
        }
    }

    #[test]
    fn scrub_redacts_whole_pem_blocks() {
        let pem = "before -----BEGIN RSA PRIVATE KEY-----\nMIIEow IBAAK CAQEA\nabc def\n-----END RSA PRIVATE KEY----- after";
        assert_eq!(red(pem), (true, "before [redacted] after".to_owned()));
        let open = "x -----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1r ZXktdjEAAAA";
        assert_eq!(red(open), (true, "x [redacted]".to_owned()));
    }

    #[test]
    fn scrub_redacts_jwts_and_high_entropy_strings() {
        let jwt = "bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        assert_eq!(red(jwt), (true, "bearer [redacted]".to_owned()));
        let (hit, _) = red("blob Zm9vYmFyYmF6cXV4M2tKZGw5UXpXcTdOeGI0UmY=");
        assert!(hit, "base64 secret");
        // low entropy, long words and paths stay
        for keep in [
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            "internationalizationlocalizations",
            "uploaded bank statement",
        ] {
            assert_eq!(red(keep), (false, keep.to_owned()), "{keep}");
        }
    }

    #[test]
    fn scrub_catches_prefixes_glued_to_punctuation() {
        let tail = "abcdefghijklmnop1234";
        for glue in ['|', '&', '@', '/', '\\', '!', '~', '^', '+', '?', '#', '`'] {
            for p in ["ghp_", "sk-ant-", "xoxb-"] {
                let inp = format!("x{glue}{p}{tail}");
                let (hit, out) = red(&inp);
                assert!(hit && !out.contains(tail), "{inp} -> {out}");
            }
        }
        // a prefix inside a word is not a key
        assert!(!red("task-abcdefghijklmnop").0);
    }

    #[test]
    fn scrub_pem_is_case_insensitive() {
        let pem = "a -----begin private key-----\nMIIEvQ\n-----end private key----- b";
        assert_eq!(red(pem), (true, "a [redacted] b".to_owned()));
    }

    #[test]
    fn scrub_keeps_git_hashes_identifiers_and_paths() {
        for keep in [
            "commit 098dfc3be9eb2fa1b2c3d4e5f60718293a4b5c6d",
            "SomeVeryLongTypeNameForTheDashboardWidget2Builder",
            "MAX_ENROLLMENT_CODE_GUESSES_PER_MINUTE_V2",
            "crates/qrtz-dash/src/store.rs",
            "/Users/dev/code/qrtz/apps/desktop/src/Main2View.tsx",
            "src/features/DashView/WidgetRenderer2Long.tsx",
        ] {
            assert_eq!(red(keep), (false, keep.to_owned()), "{keep}");
        }
        // 41 hex characters is still redacted
        assert!(red("098dfc3be9eb2fa1b2c3d4e5f60718293a4b5c6d0").0);
    }

    #[test]
    fn scrub_value_scans_object_keys() {
        let mut v = json!({"ghp_abcdefghijklmnopqrstuvwx": 1, "ok": {"sk-abcdefghijklmnop1234": "x"}});
        assert!(scrub_value(&mut v));
        assert_eq!(v, json!({"[redacted]": 1, "ok": {"[redacted]": "x"}}));
    }

    #[test]
    fn codes_are_uniform_over_the_alphabet() {
        let mut counts = [0usize; 30];
        for _ in 0..3000 {
            for c in code_chars(8).bytes() {
                counts[CODE_ALPHABET.iter().position(|a| *a == c).unwrap()] += 1;
            }
        }
        // 24,000 draws, 800 expected each; % 30 would give the first 16 ~ 9/8 of the rest.
        let (lo, hi) = (
            counts[..16].iter().sum::<usize>() as f64 / 16.0,
            counts[16..].iter().sum::<usize>() as f64 / 14.0,
        );
        assert!((lo / hi - 1.0).abs() < 0.06, "{lo} {hi}");
    }

    #[test]
    fn names_fold_confusables_and_strip_bidi() {
        assert_eq!(clean_name("Grok\u{202E} Bot\u{200B}"), "Grok Bot");
        assert_eq!(skeleton("Adm1n"), skeleton("admin"));
        assert_eq!(skeleton("Sуstеm"), skeleton("system"));
        assert!(valid_key("state-farm") && !valid_key("State") && !valid_key("a--b") && !valid_key(""));
    }
}

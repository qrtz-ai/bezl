//! The confirm gate (ADR 0215; decision-layer spec §9; Bound → Confirm → Act). Before an action that may not be
//! undoable, the run stops with `confirm.requested` and Jared sees exactly what would happen: the action, the app
//! and the element. Approve acts once; deny ends the run.
//!
//! The rules are a **fixed table owned by the host** (role, label, app, key combo). No scorer is consulted to
//! lower it: a model may only add a reason to ask, never remove one. Labels are matched after folding (case,
//! homoglyphs from Cyrillic and Greek, full-width and mathematical letters, zero-width and combining characters,
//! letter-spaced words, digit-for-letter spellings), and a word mixing Latin with other letters is itself a reason
//! to ask, so "Ѕend", "s e n d", "SEND!!" and "s3nd" all gate.
//!
//! An approval binds to (app, element handle, action with its parameters, tree hash). Right before acting the
//! driver reads the tree again; if its hash moved the approval is void and the gate asks again, and the helper
//! re-checks the element's fingerprint at the moment it acts.
use crate::act::{normalize_combo, Action};
use crate::ax::{words, AxNode};
use serde::{Deserialize, Serialize};

/// Label words that make an action ask first: sends, purchases, deletions, quits, irreversible settings.
pub const LABEL_RULES: &[&str] = &[
    "send",
    "post",
    "submit",
    "publish",
    "tweet",
    "reply",
    "purchase",
    "buy",
    "pay",
    "checkout",
    "order",
    "subscribe",
    "unsubscribe",
    "donate",
    "transfer",
    "delete",
    "remove",
    "erase",
    "trash",
    "discard",
    "overwrite",
    "replace",
    "quit",
    "reset",
    "format",
    "uninstall",
    "install",
    "restart",
    "shutdown",
    "logout",
    "signout",
    "revoke",
    "block",
    "report",
    "allow",
    "accept",
    "approve",
    "confirm",
    "share",
    "forward",
    "deploy",
    "merge",
    "dont",
];
/// Two-word phrases folded to one rule (the rule id is the joined word).
const LABEL_PHRASES: &[(&str, &str, &str)] = &[
    ("log", "out", "logout"),
    ("sign", "out", "signout"),
    ("shut", "down", "shutdown"),
    ("don", "t", "dont"),
];

/// Words that make a text input a password or payment field: excluded from the tree entirely.
pub const SENSITIVE_WORDS: &[&str] = &[
    "password",
    "passwort",
    "passcode",
    "passphrase",
    "pin",
    "cvv",
    "cvv2",
    "cvc",
    "csc",
    "iban",
    "ssn",
    "otp",
    "2fa",
    "expiry",
    "expiration",
    "card",
    "creditcard",
    "routing",
    "swift",
    "bic",
];
const SENSITIVE_PHRASES: &[(&str, &str)] = &[
    ("security", "code"),
    ("account", "number"),
    ("social", "security"),
    ("verification", "code"),
    ("one", "time"),
];

/// Apps where any change asks (System Settings).
pub const SETTINGS_APPS: &[&str] = &["com.apple.systempreferences", "com.apple.Settings"];
/// Apps where Return in a field sends a message.
pub const MESSAGING_APPS: &[&str] = &[
    "com.apple.MobileSMS",
    "com.apple.mail",
    "com.tinyspeck.slackmacgap",
    "com.hnc.Discord",
    "net.whatsapp.WhatsApp",
    "ru.keepcoder.Telegram",
    "com.microsoft.teams2",
];
/// Apps where Return in a field runs a command.
pub const TERMINAL_APPS: &[&str] = &[
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "dev.warp.Warp-Stable",
];
/// Key combos that ask, with their rule id.
pub const COMBO_RULES: &[(&str, &str)] = &[
    ("cmd+q", "key:quit"),
    ("cmd+delete", "key:delete"),
    ("cmd+option+delete", "key:delete"),
    ("cmd+shift+delete", "key:empty_trash"),
    ("cmd+option+shift+delete", "key:empty_trash"),
    ("cmd+return", "key:send"),
    ("cmd+shift+d", "key:send"),
    ("cmd+shift+q", "key:log_out"),
    ("ctrl+cmd+q", "key:lock"),
];

fn fold_char(c: char) -> Option<char> {
    let cp = c as u32;
    // Invisible: zero-width, joiners, soft hyphen, BOM, word joiner, variation selectors; combining marks.
    if matches!(cp, 0x200B..=0x200F | 0x2060..=0x2064 | 0xFEFF | 0x00AD | 0xFE00..=0xFE0F | 0x0300..=0x036F) {
        return None;
    }
    // Full-width ASCII.
    if (0xFF01..=0xFF5E).contains(&cp) {
        return char::from_u32(cp - 0xFEE0);
    }
    // Mathematical alphanumeric letters (bold, italic, script, …): 52 letters per style.
    if (0x1D400..=0x1D6A3).contains(&cp) {
        let i = (cp - 0x1D400) % 52;
        return char::from_u32(if i < 26 {
            'a' as u32 + i
        } else {
            'a' as u32 + i - 26
        });
    }
    let m = match c {
        'а' | 'А' | 'α' | 'Α' => 'a',
        'в' | 'В' | 'β' | 'Β' => 'b',
        'с' | 'С' | 'ϲ' => 'c',
        'ԁ' => 'd',
        'е' | 'Е' | 'ё' | 'ε' | 'Ε' => 'e',
        'ɡ' => 'g',
        'һ' | 'Н' | 'Η' => 'h',
        'і' | 'І' | 'ι' | 'Ι' | 'ӏ' => 'i',
        'ј' | 'Ј' => 'j',
        'к' | 'К' | 'κ' | 'Κ' => 'k',
        'м' | 'М' | 'Μ' => 'm',
        'п' | 'ν' | 'Ν' => 'n',
        'о' | 'О' | 'ο' | 'Ο' | 'σ' => 'o',
        'р' | 'Р' | 'ρ' | 'Ρ' => 'p',
        'ԛ' => 'q',
        'ѕ' | 'Ѕ' => 's',
        'т' | 'Т' | 'τ' | 'Τ' => 't',
        'υ' | 'ս' => 'u',
        'ѵ' => 'v',
        'ԝ' | 'ω' => 'w',
        'х' | 'Х' | 'χ' | 'Χ' => 'x',
        'у' | 'У' | 'γ' => 'y',
        'Ζ' => 'z',
        c => c,
    };
    Some(m)
}

/// The label as the rules read it: lowercase, folded.
pub fn fold_label(s: &str) -> String {
    s.chars()
        .filter_map(fold_char)
        .flat_map(char::to_lowercase)
        .collect()
}

fn leet(w: &str) -> String {
    w.chars()
        .map(|c| match c {
            '0' => 'o',
            '1' => 'i',
            '3' => 'e',
            '4' => 'a',
            '5' => 's',
            '7' => 't',
            c => c,
        })
        .collect()
}

/// The words a rule matches: folded words, their digit-for-letter spellings, and runs of single letters joined
/// ("s e n d" → "send").
fn rule_words(label: &str) -> (Vec<String>, bool) {
    let folded = fold_label(label);
    let raw: Vec<String> = words(&folded).into_iter().map(|(w, _, _)| w).collect();
    let mixed = raw.iter().any(|w| {
        w.chars().any(|c| c.is_ascii_alphabetic()) && w.chars().any(|c| c.is_alphabetic() && !c.is_ascii())
    });
    let mut out: Vec<String> = Vec::new();
    let mut run = String::new();
    for w in &raw {
        out.push(w.clone());
        let l = leet(w);
        if l != *w {
            out.push(l);
        }
        if w.chars().count() == 1 {
            run.push_str(w);
        } else {
            if run.chars().count() > 1 {
                out.push(leet(&run));
            }
            run.clear();
        }
    }
    if run.chars().count() > 1 {
        out.push(leet(&run));
    }
    // Plurals and -ing / -ed forms of a rule word count ("deleting", "sends", "posted").
    let stems: Vec<String> = out
        .iter()
        .flat_map(|w| {
            ["ing", "ed", "es", "s"]
                .iter()
                .filter_map(|suf| w.strip_suffix(suf).map(str::to_string))
                .collect::<Vec<_>>()
        })
        .collect();
    out.extend(stems);
    (out, mixed)
}

/// The label rules a title matches (rule ids `label:<word>`, plus `label:mixed_script`), sorted and deduped.
pub fn label_risk(title: &str) -> Vec<String> {
    let (ws, mixed) = rule_words(title);
    let mut out: Vec<String> = LABEL_RULES
        .iter()
        .filter(|r| ws.iter().any(|w| w == *r))
        .map(|r| format!("label:{r}"))
        .collect();
    for (a, b, id) in LABEL_PHRASES {
        if ws.windows(2).any(|p| p[0] == *a && p[1] == *b) {
            out.push(format!("label:{id}"));
        }
    }
    if mixed {
        out.push("label:mixed_script".into());
    }
    out.sort();
    out.dedup();
    out
}

/// A text input whose label names a password, passcode or payment detail.
pub fn sensitive_field(label: &str) -> bool {
    let (ws, _) = rule_words(label);
    ws.iter().any(|w| SENSITIVE_WORDS.contains(&w.as_str()))
        || SENSITIVE_PHRASES
            .iter()
            .any(|(a, b)| ws.windows(2).any(|p| p[0] == *a && p[1] == *b))
}

/// The host's rules for one action: the element's label rules, the app rules and the combo rules. `extra` is what
/// a model adds (it can only add). Sorted, deduped; empty = no confirm.
pub fn rules_for(action: &Action, node: Option<&AxNode>, app: &str, extra: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if action.handle().is_some() {
        if let Some(n) = node {
            out.extend(n.risk.iter().cloned());
        }
    }
    let changes = !matches!(
        action,
        Action::OpenApp { .. } | Action::Scroll { .. } | Action::Focus { .. }
    );
    if changes && SETTINGS_APPS.contains(&app) {
        out.push("app:system_settings".into());
    }
    let submits = match action {
        Action::SetValue { submit, .. } => *submit,
        Action::KeyCombo { combo, .. } => normalize_combo(combo) == "return",
        _ => false,
    };
    if submits && MESSAGING_APPS.contains(&app) {
        out.push("app:messaging_submit".into());
    }
    if submits && TERMINAL_APPS.contains(&app) {
        out.push("app:terminal_submit".into());
    }
    if let Action::KeyCombo { combo, .. } = action {
        let c = normalize_combo(combo);
        out.extend(
            COMBO_RULES
                .iter()
                .filter(|(k, _)| *k == c)
                .map(|(_, id)| id.to_string()),
        );
    }
    out.extend(extra.iter().cloned());
    out.sort();
    out.dedup();
    out
}

/// What an approval covers: exactly this action on this element in this tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub app: String,
    pub handle: Option<String>,
    pub action: String,
    pub tree_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmAnswer {
    Approve,
    Deny,
}

/// The gate as the state holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateState {
    pub id: String,
    pub binding: Binding,
    pub rules: Vec<String>,
    pub answer: Option<ConfirmAnswer>,
    /// The re-check right before acting: `Some(true)` = same tree, same element, same action.
    pub reverified: Option<bool>,
    /// The approved action was requested: the approval is spent (approve acts once).
    pub used: bool,
}

impl GateState {
    /// This gate lets exactly `b` act now.
    pub fn allows(&self, b: &Binding) -> bool {
        self.answer == Some(ConfirmAnswer::Approve)
            && self.reverified == Some(true)
            && !self.used
            && self.binding == *b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_catches_homoglyphs_spacing_and_leet() {
        for l in [
            "Send",
            "SEND!!",
            "Ѕend",
            "Sеnd",
            "s e n d",
            "s3nd",
            "S\u{200B}end",
            "Ｓｅｎｄ",
            "𝐒𝐞𝐧𝐝",
            "sending",
        ] {
            assert!(
                label_risk(l).contains(&"label:send".to_string()),
                "{l:?}: {:?}",
                label_risk(l)
            );
        }
        assert!(label_risk("Sеnd").contains(&"label:send".into()));
        assert_eq!(
            label_risk("Sеnd mеssage").len(),
            1,
            "fully folded: one rule, no mixed-script flag left"
        );
        assert!(label_risk("Sжnd").contains(&"label:mixed_script".into()));
        assert!(label_risk("Log Out").contains(&"label:logout".into()));
        assert!(label_risk("Don't Save").contains(&"label:dont".into()));
        assert!(label_risk("new note").is_empty());
        assert!(
            label_risk("sender").is_empty(),
            "a longer word isn't the rule word"
        );
    }

    #[test]
    fn password_and_payment_labels_are_sensitive() {
        for l in [
            "Password",
            "pаssword",
            "Card Number",
            "CVC",
            "Security code",
            "IBAN",
            "one-time code",
        ] {
            assert!(sensitive_field(l), "{l}");
        }
        assert!(!sensitive_field("search") && !sensitive_field("address and search"));
    }
}

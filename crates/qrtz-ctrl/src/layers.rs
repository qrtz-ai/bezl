//! The option sources of the decision layers (slice 4, ADR 0106): for each node of the chain, the enumerated
//! options and their heuristic priors. Pure: the goal text, the observed app catalog (`obs.apps`) and the choices
//! so far in, the next question (or the finished plan) out. The same function rebuilds every question on replay.
//!
//! Priors are milli-logits: keyword overlap with the goal, the default browser first, a named app, running apps,
//! the ladder's rung (no-UI routes first) and a penalty for focus-intent shortcuts the ladder refuses in the
//! background. "none of these" is always last at a fixed prior, so an unclear goal asks instead of guessing.
//!
//! App names never reach the log: `obs.apps` carries bundle ids and hashed name tokens, and the goal's tokens are
//! hashed the same way to match them.
use crate::ax::{role_slug, AxNode, AxSnapshot};
use crate::decide::{Goal, LadderHint, Node, Opt, OptionKind, MAX_OPTIONS, NONE};
use crate::log::PlanStep;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// The fixed prior of "none of these".
pub const NONE_PRIOR: i64 = 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub opt: Opt,
    pub prior: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub node: Node,
    pub choices: Vec<Choice>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    Ask(Question),
    Plan(Vec<PlanStep>),
    /// The next question needs an AX observation first (slice 5): the helper snapshots `app`'s front window
    /// (`scope: None`) or one element's children, the chain records `obs.snapshot`, and asks again.
    Observe {
        app: String,
        scope: Option<String>,
    },
}

// ---------------------------------------------------------------------------------------------------------------
// Tokens

/// A lowercase word of the goal and its byte span in the original text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tok {
    pub w: String,
    pub start: usize,
    pub end: usize,
}

pub fn tokens(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let is_word = |c: char| c.is_alphanumeric() || c == '.' || c == '-' || c == '\'';
    let mut push = |s: usize, e: usize| {
        let raw = &text[s..e];
        let trimmed = raw.trim_end_matches(['.', '-', '\'']);
        let lead = trimmed.len() - trimmed.trim_start_matches(['.', '-', '\'']).len();
        let w = &trimmed[lead..];
        if !w.is_empty() {
            out.push(Tok {
                w: w.to_lowercase(),
                start: s + lead,
                end: s + lead + w.len(),
            });
        }
    };
    for (i, c) in text.char_indices() {
        match (is_word(c), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                push(s, i);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        push(s, text.len());
    }
    out
}

/// Hash of a lowercase name token: how app names are matched without being logged.
pub fn tok_hash(w: &str) -> String {
    let h = Sha256::digest(format!("qrtz-ctrl-tok:{w}").as_bytes());
    hex::encode(&h[..6])
}

/// Words that never identify an app or a target.
const STOP: &[&str] = &[
    "a", "an", "the", "my", "all", "app", "and", "to", "in", "of", "for", "on", "at", "every", "each", "day",
    "week", "it", "up", "me", "please", "open", "close", "quit",
];

fn has(toks: &[Tok], words: &[&str]) -> bool {
    toks.iter().any(|t| words.contains(&t.w.as_str()))
}

fn count(toks: &[Tok], words: &[&str]) -> i64 {
    toks.iter().filter(|t| words.contains(&t.w.as_str())).count() as i64
}

/// Words that end the object of a verb ("open youtube every morning", "… and then …").
const SPAN_STOP: &[&str] = &[
    "every", "each", "at", "daily", "weekly", "on", "tomorrow", "tonight", "and", "then", "in", "with",
    "using",
];
const SPAN_SKIP: &[&str] = &["to", "the", "my", "up", "for", "a", "an"];

/// The object of the first of `verbs`: the token range after it, fillers skipped, up to a stop word.
fn object_span(toks: &[Tok], verbs: &[&str]) -> Option<(usize, usize)> {
    let v = toks.iter().position(|t| verbs.contains(&t.w.as_str()))?;
    let mut i = v + 1;
    while i < toks.len() && SPAN_SKIP.contains(&toks[i].w.as_str()) {
        i += 1;
    }
    let s = i;
    while i < toks.len() && !SPAN_STOP.contains(&toks[i].w.as_str()) {
        i += 1;
    }
    (i > s).then_some((s, i))
}

fn is_domain(w: &str) -> bool {
    match w.rsplit_once('.') {
        Some((head, tld)) => {
            !head.is_empty() && (2..=6).contains(&tld.len()) && tld.chars().all(|c| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// Sites a bare name most likely means (`open youtube` → `youtube.com`).
const KNOWN_SITES: &[&str] = &[
    "youtube",
    "gmail",
    "google",
    "github",
    "reddit",
    "twitter",
    "netflix",
    "amazon",
    "wikipedia",
    "facebook",
    "instagram",
    "linkedin",
    "spotify",
    "twitch",
    "figma",
    "notion",
    "maps",
    "gcal",
];

/// Names whose site isn't `<name>.com` (ADR 0128): the span's words, lowercase, space-joined → the address.
const SITE_ALIASES: &[(&str, &str)] = &[
    ("google calendar", "calendar.google.com"),
    ("gcal", "calendar.google.com"),
    ("outlook calendar", "outlook.office.com/calendar"),
];

/// One or two adjacent words of the goal name an aliased site ("google calendar").
fn names_alias(toks: &[Tok]) -> bool {
    (0..toks.len()).any(|i| {
        site_alias(&toks[i].w).is_some()
            || toks
                .get(i + 1)
                .is_some_and(|n| site_alias(&format!("{} {}", toks[i].w, n.w)).is_some())
    })
}

/// The goal's words that can name an app: those of an aliased site ("google calendar") name the site, not Google
/// Chrome or Calendar (ADR 0128).
fn app_words(toks: &[Tok]) -> Vec<Tok> {
    let mut skip = vec![false; toks.len()];
    for i in 0..toks.len() {
        if site_alias(&toks[i].w).is_some() {
            skip[i] = true;
        } else if let Some(n) = toks.get(i + 1) {
            if site_alias(&format!("{} {}", toks[i].w, n.w)).is_some() {
                skip[i] = true;
                skip[i + 1] = true;
            }
        }
    }
    toks.iter()
        .zip(skip)
        .filter(|(_, s)| !s)
        .map(|(t, _)| t.clone())
        .collect()
}

/// The address a name means, if it has an alias.
pub fn site_alias(span: &str) -> Option<&'static str> {
    let key = span
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    SITE_ALIASES.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

// ---------------------------------------------------------------------------------------------------------------
// The app catalog (`obs.apps`)

/// An installed (or running) app as the layers see it: bundle id, hashed name tokens, browser or not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppObs {
    pub id: String,
    pub tokens: Vec<String>,
    pub browser: bool,
    pub running: bool,
}

/// What `obs.apps` records: the catalog the chain chose from, so a replay chooses from the same one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppsObs {
    pub apps: Vec<AppObs>,
    /// The default `https` handler, if known (the helper reports it from slice 5). Unknown → the symbolic
    /// `app.default_browser` option, resolved at execution.
    pub default_browser: Option<String>,
    /// The catalog came from the helper, which can also snapshot AX trees (slice 5): the move question then
    /// observes the app's front window first. Absent (false) in older logs, which replay unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ax: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub browser: bool,
    pub running: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppCatalog {
    pub apps: Vec<AppEntry>,
    pub default_browser: Option<String>,
    /// From the helper (LaunchServices + AX), see [`AppsObs::ax`].
    pub ax: bool,
}

impl AppCatalog {
    /// The log form: names become hashed tokens (stop words dropped), sorted by id.
    pub fn observe(&self) -> AppsObs {
        let mut apps: Vec<AppObs> = self
            .apps
            .iter()
            .map(|a| AppObs {
                id: a.id.clone(),
                tokens: tokens(&a.name)
                    .into_iter()
                    .filter(|t| !STOP.contains(&t.w.as_str()))
                    .map(|t| tok_hash(&t.w))
                    .collect(),
                browser: a.browser,
                running: a.running,
            })
            .collect();
        apps.sort_by(|a, b| a.id.cmp(&b.id));
        apps.dedup_by(|a, b| a.id == b.id);
        AppsObs {
            apps,
            default_browser: self.default_browser.clone(),
            ax: self.ax,
        }
    }
}

/// Where the catalog comes from. The helper's LaunchServices/NSWorkspace source (running apps, default browser)
/// arrives with slice 5; until then [`ScanApps`] reads the application folders.
pub trait AppSource: Send + Sync {
    fn catalog(&self) -> AppCatalog;
}

/// A fixed catalog (tests, and the e2e app).
pub struct FixedApps(pub AppCatalog);
impl AppSource for FixedApps {
    fn catalog(&self) -> AppCatalog {
        self.0.clone()
    }
}

/// Well-known macOS apps: display name → (bundle id, browser).
pub const KNOWN_APPS: &[(&str, &str, bool)] = &[
    ("Safari", "com.apple.Safari", true),
    ("Google Chrome", "com.google.Chrome", true),
    ("Firefox", "org.mozilla.firefox", true),
    ("Arc", "company.thebrowser.Browser", true),
    ("Brave Browser", "com.brave.Browser", true),
    ("Microsoft Edge", "com.microsoft.edgemac", true),
    ("Opera", "com.operasoftware.Opera", true),
    ("Vivaldi", "com.vivaldi.Vivaldi", true),
    ("Orion", "com.kagi.kagimacOS", true),
    ("Zen", "app.zen-browser.zen", true),
    ("Chromium", "org.chromium.Chromium", true),
    ("DuckDuckGo", "com.duckduckgo.macos.browser", true),
    ("Finder", "com.apple.finder", false),
    ("System Settings", "com.apple.systempreferences", false),
    ("Notes", "com.apple.Notes", false),
    ("Mail", "com.apple.mail", false),
    ("Messages", "com.apple.MobileSMS", false),
    ("Music", "com.apple.Music", false),
    ("Calendar", "com.apple.iCal", false),
    ("Terminal", "com.apple.Terminal", false),
    ("TextEdit", "com.apple.TextEdit", false),
    ("Pages", "com.apple.iWork.Pages", false),
    ("Clock", "com.apple.clock", false),
    ("Slack", "com.tinyspeck.slackmacgap", false),
];

/// Finder and System Settings live outside the application folders; every Mac has them.
const ALWAYS: &[&str] = &["Finder", "System Settings"];

fn slug(name: &str) -> String {
    tokens(name)
        .into_iter()
        .map(|t| t.w)
        .collect::<Vec<_>>()
        .join("-")
}

/// An app entry from a bundle's display name: a known bundle id, else `name.<slug>`.
pub fn app_entry(name: &str) -> AppEntry {
    match KNOWN_APPS.iter().find(|(n, ..)| n.eq_ignore_ascii_case(name)) {
        Some((n, id, browser)) => AppEntry {
            id: (*id).into(),
            name: (*n).into(),
            browser: *browser,
            running: false,
        },
        None => AppEntry {
            id: format!("name.{}", slug(name)),
            name: name.into(),
            browser: false,
            running: false,
        },
    }
}

/// Lists `*.app` bundles in the application folders (read-only, names only).
pub struct ScanApps {
    pub dirs: Vec<PathBuf>,
}

impl ScanApps {
    pub fn mac_default() -> Self {
        let mut dirs: Vec<PathBuf> = [
            "/Applications",
            "/System/Applications",
            "/System/Applications/Utilities",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Applications"));
        }
        ScanApps { dirs }
    }
}

impl AppSource for ScanApps {
    fn catalog(&self) -> AppCatalog {
        let mut names: BTreeSet<String> = ALWAYS.iter().map(|s| s.to_string()).collect();
        for d in &self.dirs {
            let Ok(rd) = std::fs::read_dir(d) else {
                continue;
            };
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if let Some(stem) = n.strip_suffix(".app") {
                    names.insert(stem.to_string());
                }
            }
        }
        AppCatalog {
            apps: names.iter().map(|n| app_entry(n)).collect(),
            default_browser: None,
            ax: false,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Intents

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppMode {
    /// No app question.
    None,
    /// Browsers: the default browser first, then the others (a named one wins).
    Browser,
    /// Apps matched by name; `fallback` gets a prior when none is named.
    Named {
        fallback: Option<&'static str>,
    },
    /// Plain file moves (rung 1) or Finder.
    Files,
    Fixed(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    None,
    Folder,
    TabScope,
    Url,
    SearchText,
    /// ADR 0140: a volume level, 0–100 (`level.N`), read from the task's own number.
    Level,
    /// ADR 0143: a relative volume step, 1–100 (`step.N`), "by N" in the task or the default.
    Step,
}

struct IntentDef {
    id: &'static str,
    app: AppMode,
    slot: Slot,
    irreversible: bool,
}

const INTENTS: &[IntentDef] = &[
    IntentDef {
        id: "clean_folder",
        app: AppMode::Files,
        slot: Slot::Folder,
        irreversible: false,
    },
    IntentDef {
        id: "close_tabs",
        app: AppMode::Browser,
        slot: Slot::TabScope,
        irreversible: false,
    },
    IntentDef {
        id: "close_windows",
        app: AppMode::Named { fallback: None },
        slot: Slot::None,
        irreversible: false,
    },
    IntentDef {
        id: "open_site",
        app: AppMode::Browser,
        slot: Slot::Url,
        irreversible: false,
    },
    IntentDef {
        id: "open_app",
        app: AppMode::Named { fallback: None },
        slot: Slot::None,
        irreversible: false,
    },
    // Quitting can lose unsaved work (spec §9).
    IntentDef {
        id: "quit_app",
        app: AppMode::Named { fallback: None },
        slot: Slot::None,
        irreversible: true,
    },
    // ADR 0130: quit the running apps not used today (the helper's last-used record), behind the gate.
    IntentDef {
        id: "quit_unused",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: true,
    },
    // ADR 0134: hide every other running app (never the one the user is in); reversible, no gate.
    IntentDef {
        id: "hide_others",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0135: mute the Mac's sound output; reversible, no gate, no app.
    IntentDef {
        id: "mute",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0139: unmute it again (the reverse of mute).
    IntentDef {
        id: "unmute",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0140: set the output volume to a level; reversible (set it again), no gate, no app.
    IntentDef {
        id: "set_volume",
        app: AppMode::None,
        slot: Slot::Level,
        irreversible: false,
    },
    // ADR 0143: turn the output volume up or down by a step from the level read before; reversible, no gate.
    IntentDef {
        id: "change_volume",
        app: AppMode::None,
        slot: Slot::Step,
        irreversible: false,
    },
    IntentDef {
        id: "empty_trash",
        app: AppMode::Files,
        slot: Slot::None,
        irreversible: true,
    },
    // ADR 0137: the system appearance, through System Events (faceless); reversible (the opposite task undoes it),
    // so no gate.
    IntentDef {
        id: "dark_mode",
        app: AppMode::Fixed("com.apple.systemevents"),
        slot: Slot::None,
        irreversible: false,
    },
    IntentDef {
        id: "search",
        app: AppMode::Named {
            fallback: Some("app.default_browser"),
        },
        slot: Slot::SearchText,
        irreversible: false,
    },
    IntentDef {
        id: "send_message",
        app: AppMode::Named {
            fallback: Some("com.apple.MobileSMS"),
        },
        slot: Slot::None,
        irreversible: true,
    },
];

fn intent_def(id: &str) -> Option<&'static IntentDef> {
    INTENTS.iter().find(|d| d.id == id)
}

/// ADR 0138: the words that mean "my email" (the Mail app when no mail app is named).
pub const MAIL_APP: &str = "com.apple.mail";
fn mail_words(toks: &[Tok]) -> bool {
    has(toks, &["email", "emails", "inbox", "e-mail", "mail"])
}

const FOLDER_WORDS: &[&str] = &[
    "downloads",
    "download",
    "desktop",
    "documents",
    "docs",
    "screenshots",
    "folder",
];

/// Keyword evidence for an intent, milli-logits.
fn intent_prior(id: &str, toks: &[Tok], apps: &AppsObs) -> i64 {
    let named = named_apps(&app_words(toks), apps);
    let named_app = !named.is_empty();
    // An app named in full by two or more words ("notion calendar", "google chrome") outweighs a site word in it.
    let full_named = named
        .iter()
        .any(|(a, h)| *h >= 2 && *h as usize == a.tokens.len());
    let site = has(toks, KNOWN_SITES) || toks.iter().any(|t| is_domain(&t.w)) || names_alias(toks);
    let open = has(toks, &["open", "go", "visit", "load", "launch", "start", "show"]);
    let close = has(toks, &["close", "shut"]);
    let mut p = 0;
    match id {
        "clean_folder" => {
            if has(
                toks,
                &[
                    "clean",
                    "tidy",
                    "organize",
                    "organise",
                    "sort",
                    "declutter",
                    "clear",
                ],
            ) {
                p += 2000;
            }
            if has(toks, FOLDER_WORDS) {
                p += 2500;
            }
        }
        "close_tabs" => {
            if has(toks, &["tab", "tabs"]) {
                p += 3000;
            }
            if close {
                p += 2000;
            }
            if has(toks, &["all"]) {
                p += 500;
            }
        }
        "close_windows" => {
            if has(toks, &["window", "windows"]) {
                p += 3000;
            }
            if close {
                p += 2000;
            }
        }
        "open_site" => {
            if open {
                p += 1500;
            }
            if site {
                p += 3000;
            }
        }
        "open_app" => {
            if open {
                p += 1500;
            }
            // ADR 0138: "open my email" / "check my inbox" means the mail app (Mail, or the one named).
            if mail_words(toks)
                && !named_app
                && (open || has(toks, &["check", "read"]))
                && !has(toks, &["send", "reply", "write"])
            {
                p += if open { 3000 } else { 4500 };
            }
            if named_app && (!site || full_named) {
                p += 3000;
            }
            if full_named && site {
                p += 2000;
            }
        }
        "quit_app" => {
            if has(toks, &["quit", "exit"]) {
                p += 3000;
            }
            if named_app && (close || has(toks, &["quit", "exit"])) {
                p += 1500;
            }
        }
        "quit_unused" => {
            // Apps as a group plus a not-used word ("quit apps i haven't used today", "quit unused apps"). Without
            // them this intent sits well below the rest, so adding it doesn't thin out the other intents' odds.
            p -= 3000;
            if has(toks, &["apps", "app", "applications", "programs"]) {
                let usage = has(
                    toks,
                    &["unused", "used", "using", "idle", "inactive", "untouched"],
                );
                if usage {
                    p += 5500;
                    if has(toks, &["quit", "exit", "close"]) {
                        p += 3000;
                    }
                }
            }
        }
        "hide_others" => {
            // "hide other apps", "hide all my apps", "hide everything else". Without "hide" it sits well below the
            // rest (like quit_unused), so it doesn't thin out the other intents' odds.
            // Only with a group word: "hide slack" names one app and must never hide everything else.
            p -= 3000;
            let group = has(
                toks,
                &[
                    "apps",
                    "applications",
                    "programs",
                    "other",
                    "others",
                    "everything",
                    "all",
                ],
            );
            if has(toks, &["hide"]) && group && !named_app {
                p += 8500;
            }
        }
        "mute" => {
            // "mute my mac", "mute the sound". Not "mute slack" (one app's own mute is a different thing).
            p -= 3000;
            if has(toks, &["mute", "silence"]) && !named_app {
                p += 8500;
            }
        }
        "unmute" => {
            // "unmute my mac", "turn the sound back on". Not "unmute slack".
            p -= 3000;
            let sound_on = has(toks, &["sound", "audio", "volume"])
                && has(toks, &["on"])
                && has(toks, &["turn", "back"]);
            if (has(toks, &["unmute", "unsilence"]) || sound_on) && !named_app {
                p += 8500;
            }
        }
        "set_volume" => {
            // "set the volume to 30", "volume 40%", "turn the sound to half". Only with a level in the task and no
            // app named ("set spotify's volume to 30" is that app's own slider, not this).
            p -= 3000;
            // ADR 0143: "turn the volume down by 5" is a step, not a level; "lower the volume to 20" is still a level.
            let relative = volume_dir(toks).is_some() && !sure_level(toks);
            if has(toks, &["volume", "sound"]) && !levels(toks).is_empty() && !named_app && !relative {
                p += 8500;
            }
        }
        "change_volume" => {
            // ADR 0143: "turn it up", "turn the volume down", "a bit louder", "volume up by 20". A direction plus the
            // volume, a louder/quieter word, or "turn it"; never with a sure level ("turn it down to 20" sets 20)
            // and never with an app named ("turn up spotify" is that app's own slider).
            p -= 3000;
            let sounds = has(toks, &["volume", "sound", "audio"])
                || has(toks, &["louder", "quieter", "softer"])
                || (has(toks, &["turn"]) && has(toks, &["it"]));
            if volume_dir(toks).is_some() && sounds && !sure_level(toks) && !named_app {
                p += 8500;
            }
        }
        "empty_trash" => {
            if has(toks, &["empty"]) {
                p += 2000;
            }
            if has(toks, &["trash", "bin"]) {
                p += 3000;
            }
        }
        "dark_mode" => {
            if has(toks, &["dark", "light"]) && has(toks, &["mode", "theme"]) {
                p += 4000;
            }
            if has(toks, &["appearance"]) {
                p += 3000;
            }
        }
        "search" => {
            if has(toks, &["search", "find", "look"]) {
                p += 3000;
            }
        }
        "send_message" => {
            if has(toks, &["send", "message", "text", "email", "reply", "dm"]) {
                p += 3000;
            }
        }
        _ => {}
    }
    p
}

// ---------------------------------------------------------------------------------------------------------------
// Option helpers

fn opt(id: impl Into<String>, kind: OptionKind) -> Opt {
    Opt {
        id: id.into(),
        kind,
        label_ref: None,
        irreversible: false,
        ladder: None,
    }
}

fn choice(o: Opt, prior: i64) -> Choice {
    Choice { opt: o, prior }
}

/// Keep the best `MAX_OPTIONS - 1` by prior (stable: ties keep source order, so the default browser stays first)
/// and end with "none of these".
fn finish(node: Node, mut cs: Vec<Choice>) -> Question {
    cs.sort_by(|a, b| b.prior.cmp(&a.prior));
    cs.truncate(MAX_OPTIONS - 1);
    cs.push(choice(opt(NONE, OptionKind::None), NONE_PRIOR));
    Question { node, choices: cs }
}

/// Apps whose name tokens all appear in the goal, best (most tokens) first.
fn named_apps<'a>(toks: &[Tok], apps: &'a AppsObs) -> Vec<(&'a AppObs, i64)> {
    let goal: BTreeSet<String> = toks
        .iter()
        .filter(|t| !STOP.contains(&t.w.as_str()))
        .map(|t| tok_hash(&t.w))
        .collect();
    let mut out: Vec<(&AppObs, i64)> = apps
        .apps
        .iter()
        .filter_map(|a| {
            let hits = a.tokens.iter().filter(|h| goal.contains(*h)).count() as i64;
            (hits > 0).then_some((a, hits))
        })
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out
}

fn app_opt(id: &str) -> Opt {
    opt(format!("app.{id}"), OptionKind::App)
}

fn app_question(def: &IntentDef, toks: &[Tok], apps: &AppsObs) -> Question {
    let named: BTreeMap<&str, i64> = named_apps(&app_words(toks), apps)
        .into_iter()
        .map(|(a, h)| (a.id.as_str(), h))
        .collect();
    let mut cs = Vec::new();
    match def.app {
        AppMode::None => {}
        AppMode::Browser => {
            // Default browser first (spec §2): the concrete one if known, else the symbolic default.
            let default = apps.default_browser.clone();
            match &default {
                Some(id) => cs.push(choice(
                    app_opt(id),
                    4000 + named.get(id.as_str()).map_or(0, |_| 2000),
                )),
                None => cs.push(choice(opt("app.default_browser", OptionKind::App), 4000)),
            }
            for a in apps
                .apps
                .iter()
                .filter(|a| a.browser && Some(&a.id) != default.as_ref())
            {
                let mut p = 500 + if a.running { 500 } else { 0 };
                if named.contains_key(a.id.as_str()) {
                    p += 5500;
                }
                cs.push(choice(app_opt(&a.id), p));
            }
        }
        AppMode::Named { fallback } => {
            // ADR 0138: "my email" with no app named is the Mac's Mail, as "my calendar" is Calendar (ADR 0128).
            if def.id == "open_app" && named.is_empty() && mail_words(toks) {
                if let Some(m) = apps.apps.iter().find(|a| a.id == MAIL_APP) {
                    cs.push(choice(app_opt(&m.id), 6000 + if m.running { 500 } else { 0 }));
                }
            }
            if let Some(f) = fallback {
                let o = if f == "app.default_browser" {
                    opt(f, OptionKind::App)
                } else {
                    app_opt(f)
                };
                let named_it = named.contains_key(f);
                cs.push(choice(o, if named_it { 0 } else { 2000 }));
            }
            for a in &apps.apps {
                if let Some(h) = named.get(a.id.as_str()) {
                    let running = if a.running { 500 } else { 0 };
                    // The app the goal names in full ("my calendar" → Calendar) over one it names in part (Notion
                    // Calendar), ADR 0128.
                    let full = if *h == a.tokens.len() as i64 { 1000 } else { 0 };
                    let p = 4000 + 1000 * (h - 1) + running + full;
                    if let Some(c) = cs.iter_mut().find(|c| c.opt.id == format!("app.{}", a.id)) {
                        c.prior = p;
                    } else {
                        cs.push(choice(app_opt(&a.id), p));
                    }
                }
            }
        }
        AppMode::Files => {
            cs.push(choice(opt("app.files", OptionKind::App), 4000));
            cs.push(choice(app_opt("com.apple.finder"), 1000));
        }
        AppMode::Fixed(id) => cs.push(choice(app_opt(id), 4000)),
    }
    finish(Node::ChooseApp, cs)
}

fn folder_question(toks: &[Tok]) -> Question {
    let folders: [(&str, &[&str]); 4] = [
        ("downloads", &["downloads", "download"]),
        ("desktop", &["desktop"]),
        ("documents", &["documents", "docs"]),
        ("screenshots", &["screenshots", "screenshot"]),
    ];
    // "my screenshots on the desktop" names one place, not two (ADR 0124): the Desktop is where macOS saves them.
    let on_desktop = has(toks, &["desktop"]);
    let cs = folders
        .iter()
        .map(|(id, words)| {
            let named = count(toks, words).min(1);
            let p = if *id == "screenshots" && on_desktop {
                0
            } else {
                5000 * named
            };
            choice(opt(format!("folder.{id}"), OptionKind::Folder), p)
        })
        .collect();
    finish(Node::ChooseTarget, cs)
}

fn scope_question(toks: &[Tok]) -> Question {
    let plural = has(toks, &["tabs"]);
    let all = 1500 + if has(toks, &["all"]) { 4000 } else { 0 } + if plural { 1000 } else { 0 };
    let front =
        500 + if has(toks, &["this", "current", "front"]) {
            4000
        } else {
            0
        } + if has(toks, &["tab"]) { 1000 } else { 0 };
    let other = if has(toks, &["other", "others"]) { 4500 } else { 0 };
    finish(
        Node::ChooseTarget,
        vec![
            choice(opt("scope.all_tabs", OptionKind::Scope), all),
            choice(opt("scope.front_tab", OptionKind::Scope), front),
            choice(opt("scope.other_tabs", OptionKind::Scope), other),
        ],
    )
}

/// ADR 0140: the volume levels a task names, with how sure each is. A number right after "to" / "volume" or before
/// "%" / "percent" is the level; one after "at" or before "am" / "pm" / ":" is a time ("every day at 8"), never
/// offered. "half" is 50, "max" / "full" / "maximum" 100, "zero" / "silent" 0. Only 0–100.
fn levels(toks: &[Tok]) -> Vec<(u8, i64)> {
    let mut out: Vec<(u8, i64)> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let word = match t.w.as_str() {
            "half" => Some(50),
            "max" | "maximum" | "full" => Some(100),
            "zero" => Some(0),
            _ => None,
        };
        let n: Option<u8> = word.or_else(|| {
            (t.w.chars().all(|c| c.is_ascii_digit()) && t.w.len() <= 3)
                .then(|| t.w.parse().ok())
                .flatten()
        });
        let Some(n) = n.filter(|n| *n <= 100) else {
            continue;
        };
        let prev = i.checked_sub(1).map(|j| toks[j].w.as_str());
        let next = toks.get(i + 1).map(|n| n.w.as_str());
        let timeish = prev == Some("at")
            || matches!(next, Some("am" | "pm" | "o'clock"))
            || (i > 0
                && toks[i - 1].end + 1 == t.start
                && prev.is_some_and(|p| p.chars().all(|c| c.is_ascii_digit())))
            || toks
                .get(i + 1)
                .is_some_and(|nx| nx.start == t.end + 1 && nx.w.chars().all(|c| c.is_ascii_digit()));
        if timeish {
            continue;
        }
        let mut p = 2000;
        if matches!(prev, Some("to" | "volume" | "sound")) {
            p += 3000;
        }
        if matches!(next, Some("percent" | "%")) || t.w.parse::<u8>().is_ok() && next.is_none() {
            p += 1000;
        }
        if word.is_some() && !matches!(prev, Some("to" | "volume" | "sound")) {
            p -= 1000;
        }
        match out.iter_mut().find(|(m, _)| *m == n) {
            Some(e) => e.1 = e.1.max(p),
            None => out.push((n, p)),
        }
    }
    // ADR 0143: "all the way up / down" is the end of the range, a sure level (not a step).
    for w in toks.windows(4) {
        let n = match (w[0].w.as_str(), w[1].w.as_str(), w[2].w.as_str(), w[3].w.as_str()) {
            ("all", "the", "way", "up") => 100,
            ("all", "the", "way", "down") => 0,
            _ => continue,
        };
        match out.iter_mut().find(|(m, _)| *m == n) {
            Some(e) => e.1 = e.1.max(5000),
            None => out.push((n, 5000)),
        }
    }
    out
}

/// ADR 0143: the direction of a relative volume change: up (`true`), down (`false`), or none / both (`None`).
fn volume_dir(toks: &[Tok]) -> Option<bool> {
    let up = has(toks, &["up", "louder", "raise", "increase", "higher", "boost"]);
    let down = has(
        toks,
        &["down", "quieter", "softer", "lower", "decrease", "reduce"],
    );
    match (up, down) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// ADR 0143: the task names a level outright (a number after "to", "volume" or "sound").
fn sure_level(toks: &[Tok]) -> bool {
    levels(toks).iter().any(|(_, p)| *p >= 5000)
}

/// ADR 0143: the step of a relative change: a number right after "by" or after the direction word ("up 20"),
/// 1–100; else "a bit" / "a little" / "slightly" is the small step; else the default. Two different numbers ask.
fn step_question(toks: &[Tok]) -> Question {
    let mut steps: Vec<u8> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| toks[j].w.as_str());
        if !matches!(prev, Some("by" | "up" | "down")) {
            continue;
        }
        let n = (t.w.chars().all(|c| c.is_ascii_digit()) && t.w.len() <= 3)
            .then(|| t.w.parse::<u8>().ok())
            .flatten();
        if let Some(n) = n.filter(|n| (1..=100).contains(n)) {
            if !steps.contains(&n) {
                steps.push(n);
            }
        }
    }
    let small = has(toks, &["bit", "little", "slightly", "tad"]);
    let pick = match steps.as_slice() {
        [] if small => vec![(crate::apps::SMALL_VOLUME_STEP, 6000)],
        [] => vec![(crate::apps::DEFAULT_VOLUME_STEP, 6000)],
        [n] => vec![(*n, 6000)],
        many => many.iter().map(|n| (*n, 2500)).collect(),
    };
    let cs = pick
        .into_iter()
        .map(|(n, p)| {
            choice(
                opt(format!("{}{n}", crate::apps::STEP_PREFIX), OptionKind::Scope),
                p,
            )
        })
        .collect();
    finish(Node::ChooseTarget, cs)
}

fn level_question(toks: &[Tok]) -> Question {
    let mut cs: Vec<Choice> = levels(toks)
        .into_iter()
        .map(|(n, p)| {
            choice(
                opt(format!("{}{n}", crate::apps::LEVEL_PREFIX), OptionKind::Scope),
                p,
            )
        })
        .collect();
    // Two numbers, neither clearly the level: neither is sure (the run asks).
    if cs.len() > 1 && cs.iter().filter(|c| c.prior >= 5000).count() != 1 {
        for c in &mut cs {
            c.prior = c.prior.min(2500);
        }
    }
    finish(Node::ChooseTarget, cs)
}

fn span_ref(goal: &Goal, toks: &[Tok], s: usize, e: usize) -> String {
    format!("{}#{}..{}", goal.task_id, toks[s].start, toks[e - 1].end)
}

fn text_opt(id: &str, label_ref: String) -> Opt {
    Opt {
        label_ref: Some(label_ref),
        ..opt(id, OptionKind::TextCandidate)
    }
}

fn url_question(goal: &Goal, toks: &[Tok]) -> Question {
    let mut cs = Vec::new();
    if let Some((s, e)) = object_span(toks, &["open", "go", "visit", "load", "show"]) {
        if let Some(i) = (s..e).find(|&i| is_domain(&toks[i].w)) {
            cs.push(choice(
                text_opt("text.literal", span_ref(goal, toks, i, i + 1)),
                5000,
            ));
        } else {
            let words: Vec<&str> = toks[s..e].iter().map(|t| t.w.as_str()).collect();
            let alias = site_alias(&words.join(" ")).is_some();
            let known = e - s == 1 && KNOWN_SITES.contains(&words[0]);
            cs.push(choice(
                text_opt("text.url_guess", span_ref(goal, toks, s, e)),
                if alias {
                    3500
                } else if known {
                    3000
                } else {
                    1500
                },
            ));
        }
        cs.push(choice(text_opt("text.search", span_ref(goal, toks, s, e)), 1000));
    }
    finish(Node::ChooseText, cs)
}

fn search_question(goal: &Goal, toks: &[Tok]) -> Question {
    let mut cs = Vec::new();
    if let Some((s, e)) = object_span(toks, &["search", "find", "look"]) {
        cs.push(choice(text_opt("text.search", span_ref(goal, toks, s, e)), 4000));
    }
    finish(Node::ChooseText, cs)
}

fn shortcut(combo: &str, prior: i64) -> Choice {
    let mut o = opt(format!("shortcut.{combo}"), OptionKind::Shortcut);
    if crate::policy::is_focus_intent(combo) {
        o.ladder = Some(LadderHint::FocusIntent);
    }
    // The ladder refuses focus-intent shortcuts in the background (ADR 0100): offered, scored down.
    let penalty = if o.ladder.is_some() { 1000 } else { 0 };
    choice(o, prior - penalty)
}

fn route(id: &str, prior: i64) -> Choice {
    choice(opt(format!("route.{id}"), OptionKind::Route), prior)
}

fn irreversible(mut c: Choice) -> Choice {
    c.opt.irreversible = true;
    c
}

fn move_question(
    intent: &str,
    toks: &[Tok],
    chosen: &BTreeMap<Node, String>,
    snap: Option<&AxSnapshot>,
) -> Question {
    let target = chosen.get(&Node::ChooseTarget).map(String::as_str);
    let mut cs = match intent {
        "close_tabs" => {
            let all = target == Some("scope.all_tabs");
            let front = target == Some("scope.front_tab");
            let other = target == Some("scope.other_tabs");
            vec![
                route("scripting.close_all_tabs", 2500 + if all { 2500 } else { 0 }),
                route("scripting.close_tab", 1000 + if front { 2500 } else { 0 }),
                shortcut("cmd+w", 1000 + if front { 1500 } else { 0 }),
                shortcut("cmd+option+w", if other { 3000 } else { 0 }),
            ]
        }
        "close_windows" => vec![route("scripting.close_window", 3500), shortcut("cmd+w", 1500)],
        "open_site" => vec![
            route("workspace.open_url", 4500),
            shortcut("cmd+l", 1500),
            shortcut("cmd+t", 500),
        ],
        "open_app" => vec![route("workspace.launch_hidden", 4500)],
        "quit_app" => vec![irreversible(shortcut("cmd+q", 4000))],
        "quit_unused" => vec![irreversible(route("workspace.quit_unused", 4500))],
        "hide_others" => vec![route("workspace.hide_others", 4500)],
        "mute" => vec![route("system.mute", 4500)],
        "unmute" => vec![route("system.unmute", 4500)],
        "set_volume" => vec![route("system.set_volume", 4500)],
        // ADR 0143: only the direction the task says, so a failed verify can never backtrack into the other one.
        "change_volume" => match volume_dir(toks) {
            Some(true) => vec![route("system.volume_up", 4500)],
            Some(false) => vec![route("system.volume_down", 4500)],
            None => vec![route("system.volume_up", 2000), route("system.volume_down", 2000)],
        },
        // ADR 0124: on the Desktop only the screenshots and screen recordings are ctrl's; the Downloads routes
        // would act on another folder, so they aren't offered here.
        "clean_folder" if matches!(target, Some("folder.desktop" | "folder.screenshots")) => {
            let trash = has(toks, &["delete", "trash", "remove", "bin"]);
            vec![
                route("files.desktop_screenshots", 4500),
                irreversible(route(
                    "files.trash_screenshots",
                    500 + if trash { 4500 } else { 0 },
                )),
            ]
        }
        // No routes for the Documents folder yet: the Downloads ones would act on another folder (ADR 0124), so its
        // one move is a hold that says so (ADR 0125) instead of an ask with nothing to pick.
        "clean_folder" if target == Some("folder.documents") => vec![route("files.documents_hold", 4500)],
        "clean_folder" => {
            let sort = has(toks, &["sort", "organize", "organise"]);
            let old = has(toks, &["old", "delete", "trash", "remove"]);
            let clean = has(toks, &["clean", "tidy", "declutter", "clear"]);
            vec![
                route("files.tidy", 3000 + if clean { 1500 } else { 0 }),
                route("files.sort_by_kind", 1000 + if sort { 3500 } else { 0 }),
                irreversible(route("files.trash_old", 500 + if old { 3500 } else { 0 })),
            ]
        }
        "empty_trash" => vec![
            irreversible(route("files.empty_trash", 4000)),
            irreversible(shortcut("cmd+shift+delete", 1500)),
        ],
        // ADR 0137: on or off from the words ("light mode", "turn off dark mode" → off); "toggle" alone asks.
        "dark_mode" => {
            let off = has(toks, &["light", "off", "disable"]);
            let on = !off && has(toks, &["dark", "on", "enable"]) && !has(toks, &["toggle"]);
            vec![
                route("scripting.dark_mode_on", if on { 4500 } else { 2000 }),
                route("scripting.dark_mode_off", if off { 4500 } else { 2000 }),
            ]
        }
        "search" => vec![shortcut("cmd+f", 3000)],
        "send_message" => vec![shortcut("cmd+n", 3000), irreversible(shortcut("cmd+return", 0))],
        _ => vec![],
    };
    // The front window's top-level branches join the shortcuts and routes (slice 5, ADR 0108).
    if let Some(snap) = snap {
        cs.extend(branch_choices(intent, toks, snap));
    }
    finish(Node::ChooseMove, cs)
}

/// The AX option id: `ax.<element handle>.<role slug>` (the slug is only for the label).
pub fn branch_id(n: &AxNode) -> String {
    format!("ax.{}.{}", n.id, role_slug(n))
}

/// The element handle of an `ax.` option id.
pub fn branch_element(option: &str) -> Option<&str> {
    option.strip_prefix("ax.")?.split_once('.').map(|(e, _)| e)
}

/// Heuristic priors for AX elements: title overlap with the goal, the window's close button for "close windows",
/// pressable leaves a little over panes. Below the scripting routes, so a script verb still wins when it exists.
fn branch_choices(intent: &str, toks: &[Tok], snap: &AxSnapshot) -> Vec<Choice> {
    let goal: BTreeSet<String> = toks
        .iter()
        .filter(|t| !STOP.contains(&t.w.as_str()))
        .map(|t| tok_hash(&t.w))
        .collect();
    snap.nodes
        .iter()
        .map(|n| {
            let overlap = n.tokens.iter().filter(|h| goal.contains(*h)).count() as i64;
            let mut p = 200 + 1200 * overlap;
            if intent == "close_windows" && n.subrole.as_deref() == Some("AXCloseButton") {
                p += 3000;
            }
            if n.children == 0 && n.actions.iter().any(|a| a == "AXPress") {
                p += 300;
            }
            choice(opt(branch_id(n), OptionKind::Branch), p)
        })
        .collect()
}

/// Inside a chosen branch: its children (window/pane → control).
fn control_question(intent: &str, toks: &[Tok], snap: &AxSnapshot) -> Question {
    finish(Node::ChooseControl, branch_choices(intent, toks, snap))
}

/// The concrete app the chain chose: a bundle id, the resolved default browser, or none (files).
pub fn chosen_app(apps: &AppsObs, chosen: &BTreeMap<Node, String>) -> Option<String> {
    match chosen.get(&Node::ChooseApp)?.strip_prefix("app.")? {
        "files" => None,
        "default_browser" => apps.default_browser.clone(),
        id => Some(id.to_string()),
    }
}

/// The snapshot recorded for (app, scope), if any (the latest wins).
pub fn find_snapshot<'a>(snaps: &'a [AxSnapshot], app: &str, scope: Option<&str>) -> Option<&'a AxSnapshot> {
    snaps
        .iter()
        .rev()
        .find(|s| s.app == app && s.scope.as_deref() == scope)
}

/// The next question for a goal, given the observed apps and the choices so far; or the plan when every node the
/// intent needs has a choice. Pure: the same inputs give the same question (replay relies on it).
pub fn next_question(goal: &Goal, apps: &AppsObs, chosen: &BTreeMap<Node, String>) -> Next {
    next_question_obs(goal, apps, &[], chosen)
}

/// [`next_question`] with the AX observations recorded so far (`obs.snapshot`). With a helper catalog
/// (`apps.ax`) and a concrete app, the move question first asks to observe the app's front window; a chosen
/// branch with children asks to observe them, then asks `choose_control` over them. An observation that failed is
/// recorded as an empty snapshot, so the chain carries on with shortcuts and routes only.
pub fn next_question_obs(
    goal: &Goal,
    apps: &AppsObs,
    snaps: &[AxSnapshot],
    chosen: &BTreeMap<Node, String>,
) -> Next {
    let toks = tokens(goal.text);
    let Some(intent_id) = chosen.get(&Node::ChooseIntent) else {
        let cs = INTENTS
            .iter()
            .map(|d| {
                let mut o = opt(format!("intent.{}", d.id), OptionKind::Intent);
                o.irreversible = d.irreversible;
                choice(o, intent_prior(d.id, &toks, apps))
            })
            .collect();
        return Next::Ask(finish(Node::ChooseIntent, cs));
    };
    let Some(def) = intent_id.strip_prefix("intent.").and_then(intent_def) else {
        // An unknown intent id can only come from a newer layer version: ask again from the top.
        return Next::Plan(Vec::new());
    };
    if def.app != AppMode::None && !chosen.contains_key(&Node::ChooseApp) {
        return Next::Ask(app_question(def, &toks, apps));
    }
    let slot_node = match def.slot {
        Slot::None => None,
        Slot::Folder | Slot::TabScope | Slot::Level | Slot::Step => Some(Node::ChooseTarget),
        Slot::Url | Slot::SearchText => Some(Node::ChooseText),
    };
    if let Some(n) = slot_node {
        if !chosen.contains_key(&n) {
            return Next::Ask(match def.slot {
                Slot::Folder => folder_question(&toks),
                Slot::TabScope => scope_question(&toks),
                Slot::Url => url_question(goal, &toks),
                Slot::SearchText => search_question(goal, &toks),
                Slot::Level => level_question(&toks),
                Slot::Step => step_question(&toks),
                Slot::None => unreachable!(),
            });
        }
    }
    let app = chosen_app(apps, chosen);
    let top = app.as_deref().and_then(|a| find_snapshot(snaps, a, None));
    if !chosen.contains_key(&Node::ChooseMove) {
        if let (true, Some(a), None) = (apps.ax, app.as_deref(), top) {
            return Next::Observe {
                app: a.to_string(),
                scope: None,
            };
        }
        return Next::Ask(move_question(def.id, &toks, chosen, top));
    }
    if let (Some(a), Some(el)) = (
        app.as_deref(),
        chosen.get(&Node::ChooseMove).and_then(|m| branch_element(m)),
    ) {
        let pane = top.and_then(|t| t.node(el)).is_some_and(|n| n.children > 0);
        if pane && !chosen.contains_key(&Node::ChooseControl) {
            return match find_snapshot(snaps, a, Some(el)) {
                Some(inner) => Next::Ask(control_question(def.id, &toks, inner)),
                None => Next::Observe {
                    app: a.to_string(),
                    scope: Some(el.to_string()),
                },
            };
        }
    }
    Next::Plan(
        [
            Node::ChooseIntent,
            Node::ChooseApp,
            Node::ChooseTarget,
            Node::ChooseText,
            Node::ChooseMove,
            Node::ChooseControl,
        ]
        .into_iter()
        .filter_map(|n| {
            chosen.get(&n).map(|o| PlanStep {
                node: n,
                option: o.clone(),
            })
        })
        .collect(),
    )
}

// ---------------------------------------------------------------------------------------------------------------
// Labels (IPC only, never the log)

fn intent_label(id: &str) -> &'static str {
    match id {
        "clean_folder" => "clean up a folder",
        "close_tabs" => "close browser tabs",
        "close_windows" => "close windows",
        "open_site" => "open a website",
        "open_app" => "open an app",
        "quit_app" => "quit an app",
        "quit_unused" => "quit apps you haven't used today",
        "hide_others" => "hide your other apps",
        "mute" => "mute your mac",
        "unmute" => "unmute your mac",
        "set_volume" => "set the volume",
        "change_volume" => "turn the volume up or down",
        "empty_trash" => "empty the trash",
        "dark_mode" => "switch dark mode",
        "search" => "search in an app",
        "send_message" => "send a message",
        _ => "something else",
    }
}

fn route_label(id: &str) -> &'static str {
    match id {
        "scripting.close_all_tabs" => "close every tab by script",
        "scripting.close_tab" => "close the front tab by script",
        "scripting.close_window" => "close the front window by script",
        "scripting.set_dark_mode" => "switch appearance by script",
        "workspace.open_url" => "open the address in the background",
        "workspace.launch_hidden" => "start it hidden",
        "workspace.quit_unused" => "quit them without bringing anything forward",
        "workspace.hide_others" => "hide them, the app you're in stays",
        "system.mute" => "mute the sound output",
        "system.unmute" => "turn the sound output back on",
        "system.set_volume" => "set the sound output to that level",
        "system.volume_up" => "turn the sound output up a step",
        "system.volume_down" => "turn the sound output down a step",
        "scripting.dark_mode_on" => "switch to dark mode",
        "scripting.dark_mode_off" => "switch to light mode",
        "files.tidy" => "sort by kind, nothing deleted",
        "files.sort_by_kind" => "sort files into folders by kind",
        "files.trash_old" => "move old installers to the trash",
        "files.empty_trash" => "empty the trash",
        "files.desktop_screenshots" => "move screenshots into a folder, nothing deleted",
        "files.trash_screenshots" => "move screenshots to the trash",
        "files.documents_hold" => "leave documents as they are, not supported yet",
        _ => "a route",
    }
}

fn shortcut_label(combo: &str) -> String {
    let what = match combo {
        "cmd+w" => "close tab",
        "cmd+option+w" => "close other tabs",
        "cmd+l" => "focus address bar",
        "cmd+t" => "new tab",
        "cmd+q" => "quit",
        "cmd+shift+delete" => "empty trash",
        "cmd+f" => "find",
        "cmd+n" => "new message",
        "cmd+return" => "send",
        _ => "shortcut",
    };
    format!("{} {what}", combo.replace('+', "-"))
}

fn app_label(id: &str) -> String {
    if id == "default_browser" {
        return "your default browser".into();
    }
    if id == "files" {
        return "files (no app)".into();
    }
    if let Some(s) = id.strip_prefix("name.") {
        return s.replace('-', " ");
    }
    match KNOWN_APPS.iter().find(|(_, b, _)| *b == id) {
        Some((n, ..)) => n.to_lowercase(),
        None => id.rsplit('.').next().unwrap_or(id).to_lowercase(),
    }
}

/// The span a `label_ref` points at, if it is into this goal's text.
fn span_text<'a>(goal: &Goal<'a>, label_ref: &str) -> Option<&'a str> {
    let (task, range) = label_ref.split_once('#')?;
    if task != goal.task_id {
        return None;
    }
    let (a, b) = range.split_once("..")?;
    goal.text.get(a.parse().ok()?..b.parse().ok()?)
}

/// A lowercase label for an option (UI copy; task-derived text resolved from the goal).
pub fn label(o: &Opt, goal: Option<&Goal>) -> String {
    let (head, rest) = o.id.split_once('.').unwrap_or((o.id.as_str(), ""));
    match head {
        NONE => "none of these".into(),
        "intent" => intent_label(rest).into(),
        "app" => app_label(rest),
        "folder" => rest.into(),
        "scope" => rest.replace('_', " "),
        // ADR 0140: a volume level, a number from 0 to 100.
        "level" => format!("{rest}%"),
        // ADR 0143: a relative step, in points of the 0–100 volume.
        "step" => format!("by {rest}"),
        "route" => route_label(rest).into(),
        "shortcut" => shortcut_label(rest),
        // `ax.<handle>.<role slug>`: the element's role ("close button", "toolbar"); its title is never kept.
        "ax" => rest
            .split_once('.')
            .map_or_else(|| "an element".into(), |(_, slug)| slug.replace('_', " ")),
        "text" => {
            let span = o
                .label_ref
                .as_deref()
                .and_then(|r| goal.and_then(|g| span_text(g, r)))
                .map(str::to_lowercase);
            match (rest, span) {
                ("literal", Some(s)) => s,
                ("url_guess", Some(s)) => match site_alias(&s) {
                    Some(a) => a.to_string(),
                    None => format!("{}.com", s.replace(' ', "")),
                },
                ("search", Some(s)) => format!("search: {s}"),
                _ => "text from the task".into(),
            }
        }
        _ => o.id.to_lowercase(),
    }
}

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
    /// ADR 0166: one of the user's own folders to show (`folder.downloads` / `folder.desktop` / `folder.documents`,
    /// [`crate::ladder::UserFolder`]); never a path.
    UserFolder,
    /// ADR 0194: how long to keep the Mac awake, 1–720 minutes (`minutes.N`): the task's own duration, or an hour when
    /// it names none; a longer, vague or open-ended one is asked.
    Duration,
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
    // ADR 0149: hide one named app (never the one the user is in); reversible, no gate.
    IntentDef {
        id: "hide_app",
        app: AppMode::Named { fallback: None },
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0152: show (unhide) one named app without bringing it forward; reversible, no gate.
    IntentDef {
        id: "show_app",
        app: AppMode::Named { fallback: None },
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0187: bring an app's minimized windows back out of the Dock / minimize them. Recognised so they never turn into
    // a hidden launch that reports done (or a close-windows question); each one move is a hold that asks out until a
    // Mac probe shows an AXMinimized write raises nothing. No app question while held: the hold is the same for any app.
    IntentDef {
        id: "unminimize_app",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: false,
    },
    IntentDef {
        id: "minimize_app",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0155: turn Focus / Do Not Disturb on or off. No background route yet, so its one move is a hold that asks out.
    IntentDef {
        id: "focus_mode",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0166: show one of the user's own folders in a Finder window (nothing comes forward); reversible, no gate.
    // No app question: the app is always the Finder, set by the route, so nothing observes the Finder's AX tree.
    IntentDef {
        id: "open_folder",
        app: AppMode::None,
        slot: Slot::UserFolder,
        irreversible: false,
    },
    // ADR 0168: empty the clipboard; not undoable, but no gate (the task asked for exactly this), no app.
    IntentDef {
        id: "clear_clipboard",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0176: eject the ejectable local volumes; plugging back in undoes it, so no gate; no app.
    IntentDef {
        id: "eject_disks",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0180: put the displays to sleep (the Mac stays awake); any key wakes them, so no gate; no app.
    IntentDef {
        id: "display_sleep",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0183: put the whole Mac to sleep; any key / the mouse / the lid wakes it, nothing is closed: no gate, no app
    // (deferred while the user is typing or clicking).
    IntentDef {
        id: "system_sleep",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0191: turn the Wi-Fi radio on or off; reversible, no gate, no app (off deferred while the user is typing or
    // clicking).
    IntentDef {
        id: "wifi_power",
        app: AppMode::None,
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0194: keep the Mac (or its screen) awake for a while: one power assertion of the helper's own with a timeout;
    // reversible, no gate, no app, nothing on screen.
    IntentDef {
        id: "keep_awake",
        app: AppMode::None,
        slot: Slot::Duration,
        irreversible: false,
    },
    // ADR 0194: let it sleep again: the helper drops its own keep-awake (never anyone else's); no gate, no app.
    IntentDef {
        id: "allow_sleep",
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
    // ADR 0173: the Dock's auto-hide, through System Events (faceless), like the appearance; reversible, no gate.
    IntentDef {
        id: "dock_autohide",
        app: AppMode::Fixed("com.apple.systemevents"),
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0195: the Dock's screen edge (left / right / bottom), through System Events; reversible, no gate.
    IntentDef {
        id: "dock_edge",
        app: AppMode::Fixed("com.apple.systemevents"),
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0197: the menu bar's auto-hide, through System Events (faceless), like the Dock's; reversible, no gate.
    IntentDef {
        id: "menu_bar_autohide",
        app: AppMode::Fixed("com.apple.systemevents"),
        slot: Slot::None,
        irreversible: false,
    },
    // ADR 0178: start the screen saver, through System Events (faceless); reversible by the user, no gate.
    IntentDef {
        id: "screensaver",
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

/// ADR 0166: the words that name a folder "open my …" can show (a closed set, [`crate::ladder::UserFolder`]).
const USER_FOLDER_WORDS: &[&str] = &["downloads", "download", "desktop", "documents", "docs"];
/// Run 40 (ADR 0166 amendment 1): the media folders' words, which count only with "my" / "folder" (Music only with
/// "folder"). No "photos": that's the Photos app.
const MEDIA_FOLDER_WORDS: &[&str] = &["pictures", "pics", "movies", "videos"];
/// The Music app, which "open my music" means (run 40).
const MUSIC_APP: &str = "com.apple.Music";
/// ADR 0173: the Dock. "hide the dock" means its auto-hide, never `NSRunningApplication.hide()` of the Dock.
const DOCK_APP: &str = "com.apple.dock";
/// ADR 0173: words that say the Dock should hide itself.
const DOCK_HIDE_WORDS: &[&str] = &[
    "hide",
    "hides",
    "hiding",
    "hidden",
    "autohide",
    "auto-hide",
    "autohiding",
    "auto-hiding",
    "automatically",
];
/// ADR 0173: words that say the Dock should stay on screen.
const DOCK_SHOW_WORDS: &[&str] = &["show", "shows", "shown", "showing", "visible", "unhide", "always"];
/// ADR 0173: words that turn the hiding off ("stop hiding the dock", "turn off dock auto-hide").
const DOCK_NOT_WORDS: &[&str] = &[
    "stop", "don't", "dont", "never", "off", "disable", "no", "not", "unhide",
];

/// ADR 0173: the task is about the Dock's auto-hide: "dock" plus a hide / show / on-off word. Not a toggle ("toggle
/// dock hiding" would need a read first, like ADR 0145; left out until asked for). Not an edge move (ADR 0195).
fn dock_task(toks: &[Tok]) -> bool {
    has(toks, &["dock"])
        && !has(toks, &["toggle", "flip"])
        && (has(toks, DOCK_HIDE_WORDS)
            || has(toks, DOCK_SHOW_WORDS)
            || has(toks, &["turn", "enable", "disable", "on", "off", "keep"]))
        && !dock_edge_task(toks)
        && !menu_bar_named(toks)
}

/// ADR 0197: words that name the menu bar ("menu bar", "menubar", "menu-bar").
fn menu_bar_named(toks: &[Tok]) -> bool {
    has(toks, &["menubar", "menu-bar", "menubars"]) || (has(toks, &["menu"]) && has(toks, &["bar", "bars"]))
}

/// ADR 0197: words that make a menu-bar task something else: its full-screen / desktop-only modes (macOS 13+ has four
/// "automatically hide and show" choices; ctrl sets only the one System Events exposes), the items and icons in it,
/// its look (background, transparency), a toggle (would need a read first, like ADR 0145).
const MENU_BAR_NOT_WORDS: &[&str] = &[
    "toggle",
    "flip",
    "full",
    "fullscreen",
    "full-screen",
    "desktop",
    "icon",
    "icons",
    "item",
    "items",
    "extra",
    "extras",
    "clock",
    "date",
    "battery",
    "wifi",
    "wi-fi",
    "bluetooth",
    "sound",
    "volume",
    "spotlight",
    "siri",
    "control",
    "center",
    "centre",
    "background",
    "transparent",
    "transparency",
    "tint",
    "color",
    "colour",
    "size",
    "spacing",
];

/// ADR 0197: the task is about the menu bar's auto-hide: the menu bar plus a hide / show / on-off word, no Dock (one
/// move per task: "hide the dock and the menu bar" is neither), nothing from [`MENU_BAR_NOT_WORDS`]. "hide the menu
/// bar", "always show the menu bar", "stop hiding the menu bar", "turn on menu bar auto-hide".
fn menu_bar_task(toks: &[Tok]) -> bool {
    menu_bar_wording(toks) && !has(toks, &["dock"]) && !has(toks, MENU_BAR_NOT_WORDS)
}

/// ADR 0197: the menu bar plus a hide / show / on-off word, whatever else is said. Such a task is never an open (a
/// hidden launch) of an app named beside it: "hide the dock and the menu bar" with the Dock running would otherwise
/// read as "open the Dock" and finish with nothing done.
fn menu_bar_wording(toks: &[Tok]) -> bool {
    menu_bar_named(toks)
        && (has(toks, DOCK_HIDE_WORDS)
            || has(toks, DOCK_SHOW_WORDS)
            || has(toks, &["turn", "enable", "disable", "on", "off", "keep"]))
}

/// ADR 0195: words that name a Dock screen edge (macOS has no top).
const DOCK_EDGE_WORDS: &[&str] = &["left", "right", "bottom"];

/// ADR 0195: the task is about moving the Dock to an edge: "dock" plus left / right / bottom, no hide / show /
/// autohide word (those stay `dock_autohide`), no toggle. "move the dock to the left", "put the dock on the right",
/// "dock on the bottom".
fn dock_edge_task(toks: &[Tok]) -> bool {
    has(toks, &["dock"])
        && has(toks, DOCK_EDGE_WORDS)
        && !has(toks, DOCK_HIDE_WORDS)
        && !has(toks, DOCK_SHOW_WORDS)
        && !has(
            toks,
            &[
                "toggle",
                "flip",
                "autohide",
                "auto-hide",
                "autohiding",
                "auto-hiding",
            ],
        )
}

/// ADR 0195: which edge the words name. One clear edge, or `None` when two differ (the move question asks).
fn dock_edge_of(toks: &[Tok]) -> Option<&'static str> {
    let left = has(toks, &["left"]);
    let right = has(toks, &["right"]);
    let bottom = has(toks, &["bottom"]);
    match (left, right, bottom) {
        (true, false, false) => Some("left"),
        (false, true, false) => Some("right"),
        (false, false, true) => Some("bottom"),
        _ => None,
    }
}

/// ADR 0173: whether the task wants the Dock hidden (auto-hide on). "show the dock", "always show the dock",
/// "stop hiding the dock", "turn off dock hiding" and "keep the dock visible" are off; "automatically hide and show
/// the dock" (System Settings' own words) is on.
fn dock_hides(toks: &[Tok]) -> bool {
    let hide = has(toks, DOCK_HIDE_WORDS);
    let negated = has(toks, DOCK_NOT_WORDS);
    let shows = has(toks, DOCK_SHOW_WORDS);
    if negated {
        // "turn off dock hiding", "stop hiding the dock", "don't hide the dock": off, unless only an "on" is said
        // beside a "not" ("turn on auto-hide, not always shown").
        return has(toks, &["on", "enable"]) && !has(toks, &["off", "disable", "stop", "never", "unhide"]);
    }
    hide && (!shows || has(toks, &["automatically", "autohide", "auto-hide"]))
}

/// ADR 0178: words that name the screen saver (one token or "screen" + "saver").
fn screensaver_named(toks: &[Tok]) -> bool {
    has(toks, &["screensaver", "screensavers"]) || (has(toks, &["screen"]) && has(toks, &["saver", "savers"]))
}
/// ADR 0178: words that start it.
const SCREENSAVER_START_WORDS: &[&str] = &[
    "start", "starts", "starting", "run", "runs", "running", "launch", "launches", "begin", "begins",
    "activate", "enable", "on", "turn",
];
/// ADR 0178 amendment 1 (run 47): words that stop the running one ("stop the screensaver", "quit the screen saver",
/// "dismiss the screensaver"). Not "turn off" / "disable": those read as its idle setting (never start it), which
/// ctrl doesn't change.
const SCREENSAVER_STOP_WORDS: &[&str] = &[
    "stop",
    "stops",
    "stopping",
    "quit",
    "quits",
    "exit",
    "exits",
    "end",
    "ends",
    "dismiss",
    "dismisses",
    "close",
    "closes",
];
/// ADR 0178: words that make a screensaver task something else: its setting (disable / off / never), a negation
/// ("don't start the screensaver"), locking the screen.
const SCREENSAVER_NOT_WORDS: &[&str] = &[
    "disable", "disables", "off", "never", "don't", "dont", "not", "no", "lock", "locks", "locking",
    "locked", "unlock", "unlocks", "password",
];
/// ADR 0178: "start the screensaver" / "start screensaver" / "turn on the screen saver".
fn screensaver_starts(toks: &[Tok]) -> bool {
    screensaver_named(toks)
        && has(toks, SCREENSAVER_START_WORDS)
        && !has(toks, SCREENSAVER_STOP_WORDS)
        && !has(toks, SCREENSAVER_NOT_WORDS)
}
/// ADR 0178 amendment 1: "stop the screensaver" / "quit the screen saver". A stop word with a start word ("stop the
/// screensaver from starting") is the setting again, so neither.
fn screensaver_stops(toks: &[Tok]) -> bool {
    screensaver_named(toks)
        && has(toks, SCREENSAVER_STOP_WORDS)
        && !has(
            toks,
            &[
                "start", "starts", "starting", "begin", "begins", "launch", "launches", "activate", "on",
            ],
        )
        && !has(toks, SCREENSAVER_NOT_WORDS)
}
/// ADR 0178: a screensaver task, starting or stopping it.
fn screensaver_task(toks: &[Tok]) -> bool {
    screensaver_starts(toks) || screensaver_stops(toks)
}

/// ADR 0180: words that name the display.
const DISPLAY_WORDS: &[&str] = &["display", "displays", "screen", "screens", "monitor", "monitors"];
/// ADR 0180: words that make a display task something else: the screen saver (ADR 0178), lock, Screen Time, sharing,
/// recording, screenshots, mirroring, brightness, the screen reader, notifications, or keeping it awake ("screen time"
/// is checked as a pair, so "at lunch time" still counts).
const DISPLAY_NOT_WORDS: &[&str] = &[
    // ADR 0194: "let the display sleep" lets it sleep on its own timer (allow_sleep), it doesn't sleep it now.
    "let",
    "lets",
    "allow",
    "allows",
    "saver",
    "savers",
    "screensaver",
    "screensavers",
    "lock",
    "locks",
    "locked",
    "locking",
    "sharing",
    "share",
    "recording",
    "record",
    "recorder",
    "screenshot",
    "screenshots",
    "shot",
    "shots",
    "mirror",
    "mirroring",
    "brightness",
    "bright",
    "brighter",
    "dim",
    "dimmer",
    "reader",
    "voiceover",
    "notification",
    "notifications",
    "wake",
    "wakes",
    "awake",
    "keep",
    "never",
    "prevent",
    "stop",
    "don't",
    "dont",
    "not",
    "caffeinate",
];
/// ADR 0183: words that name the whole Mac.
const MAC_WORDS: &[&str] = &[
    "mac",
    "macs",
    "macbook",
    "macbooks",
    "imac",
    "computer",
    "computers",
    "laptop",
    "laptops",
    "machine",
    "system",
];
/// ADR 0183: words that make a Mac-and-sleep task something else: the display only (ADR 0180), the screen saver, keeping
/// it awake, waking it, its sleep settings or a sleep timer, locking it, or the other power moves.
const SYSTEM_SLEEP_NOT_WORDS: &[&str] = &[
    "display",
    "displays",
    "screen",
    "screens",
    "monitor",
    "monitors",
    "screensaver",
    "saver",
    "wake",
    "wakes",
    "waking",
    "awake",
    "keep",
    "never",
    "prevent",
    "stop",
    "don't",
    "dont",
    "not",
    "caffeinate",
    "settings",
    "setting",
    "preferences",
    "timer",
    "lock",
    "locked",
    "restart",
    "reboot",
    "shutdown",
    "shut",
    "log",
    "logout",
    // ADR 0194: "let my mac sleep again" lets it sleep on its own timer (allow_sleep), it doesn't put it to sleep now.
    "let",
    "lets",
    "allow",
    "allows",
];
/// ADR 0191: words that say the Wi-Fi radio should be on ("on" itself counts only where it isn't "on my mac", see
/// [`wifi_on_said`]).
const WIFI_ON_WORDS: &[&str] = &["enable", "enabled", "activate", "reenable", "re-enable"];
/// ADR 0191: words that say it should be off.
const WIFI_OFF_WORDS: &[&str] = &["off", "disable", "disabled", "deactivate"];
/// ADR 0191: words after which "on" is a preposition, not the switch: "turn off wifi on my mac", "… on weekdays".
const WIFI_ON_PREP_NEXT: &[&str] = &[
    "my",
    "the",
    "this",
    "that",
    "your",
    "our",
    "a",
    "an",
    "weekdays",
    "weekends",
    "weekday",
    "weekend",
    "mondays",
    "tuesdays",
    "wednesdays",
    "thursdays",
    "fridays",
    "saturdays",
    "sundays",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];
/// ADR 0191: words that make a Wi-Fi task something else ctrl doesn't do: a network by name or password (free text),
/// joining / leaving / forgetting one, scanning, the hotspot, Wi-Fi calling, its settings, a toggle or restart (two
/// moves, or a read first), another radio, or a negation ("don't turn off the wifi", "keep wifi on").
const WIFI_NOT_WORDS: &[&str] = &[
    "password",
    "passwords",
    "network",
    "networks",
    "ssid",
    "name",
    "join",
    "joins",
    "connect",
    "connects",
    "connected",
    "connecting",
    "reconnect",
    "disconnect",
    "disconnects",
    "forget",
    "scan",
    "hotspot",
    "tether",
    "tethering",
    "share",
    "sharing",
    "calling",
    "calls",
    "settings",
    "setting",
    "preferences",
    "toggle",
    "flip",
    "restart",
    "reset",
    "reboot",
    "cycle",
    "bluetooth",
    "airdrop",
    "airplane",
    "ethernet",
    "vpn",
    "signal",
    "speed",
    "router",
    "don't",
    "dont",
    "never",
    "not",
    "keep",
    "stays",
    "stay",
    "leave",
];

/// ADR 0191: the task names Wi-Fi: "wifi", "wi-fi", or "wi" + "fi".
fn wifi_named(toks: &[Tok]) -> bool {
    has(toks, &["wifi", "wi-fi", "wifis"]) || toks.windows(2).any(|w| w[0].w == "wi" && w[1].w == "fi")
}

/// ADR 0191: "on" is the switch ("turn on wifi", "turn the wifi on", "wifi back on at 8") unless it starts a place or
/// a day ("turn off wifi on my mac", "… on weekdays") or begins the task ("on mondays …").
fn wifi_on_said(toks: &[Tok]) -> bool {
    if has(toks, WIFI_ON_WORDS) {
        return true;
    }
    toks.iter().enumerate().any(|(i, t)| {
        if t.w != "on" || i == 0 {
            return false;
        }
        let prev = toks[i - 1].w.as_str();
        let next = toks.get(i + 1).map(|n| n.w.as_str());
        let switch_verb = matches!(prev, "turn" | "switch" | "turns" | "switches" | "flick");
        switch_verb || !next.is_some_and(|n| WIFI_ON_PREP_NEXT.contains(&n))
    })
}

/// ADR 0191: which way the task turns Wi-Fi: `Some(true)` on, `Some(false)` off, `None` neither or both ("turn the
/// wifi off and on again" is a restart: two moves, not built).
fn wifi_dir(toks: &[Tok]) -> Option<bool> {
    match (wifi_on_said(toks), has(toks, WIFI_OFF_WORDS)) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// ADR 0191: "turn off my wifi", "turn wi-fi back on", "every night at 11 turn off the wifi", "disable wifi". Wi-Fi
/// named, exactly one direction, and none of the words that mean another thing.
fn wifi_task(toks: &[Tok]) -> bool {
    wifi_named(toks) && wifi_dir(toks).is_some() && !has(toks, WIFI_NOT_WORDS)
}

/// ADR 0194: the words of sleeping, for keep-awake and let-it-sleep tasks.
const SLEEP_WORDS: &[&str] = &["sleep", "sleeps", "sleeping", "asleep"];
/// ADR 0194: words that make a keep-awake / let-it-sleep task something ctrl doesn't do: no end ("forever", "never",
/// "until I say" or a clock time: an unbounded or clock-bound hold isn't built), a condition ("while it downloads",
/// "when I'm away"), the sleep settings, waking or locking it, the screen saver, a restart or shut down, another radio.
const KEEP_AWAKE_NOT_WORDS: &[&str] = &[
    "until",
    "till",
    "til",
    "forever",
    "always",
    "indefinitely",
    "permanently",
    "never",
    "while",
    "whilst",
    "when",
    "whenever",
    "if",
    "unless",
    "settings",
    "setting",
    "preferences",
    "schedule",
    "wake",
    "wakes",
    "waking",
    "lock",
    "locks",
    "locked",
    "unlock",
    "screensaver",
    "saver",
    "restart",
    "reboot",
    "shutdown",
    "shut",
    "logout",
    "wifi",
    "wi-fi",
    "bluetooth",
];
/// ADR 0194: words that say a duration only vaguely ("for a couple of hours", "for a bit"): asked, never guessed.
const VAGUE_DURATION_WORDS: &[&str] = &["couple", "few", "several", "bit", "little", "some", "while"];

/// ADR 0194: the task asks to let the Mac sleep again: "let my mac sleep again", "allow the computer to sleep", "stop
/// keeping my mac awake", "cancel caffeinate". Never with a negation ("don't let my mac sleep" keeps it awake) and
/// never "stop my mac from sleeping" (that keeps it awake too).
fn allow_sleep_said(toks: &[Tok]) -> bool {
    let sleep = has(toks, SLEEP_WORDS);
    let negated = has(toks, &["don't", "dont", "not", "never", "prevent", "no"]);
    let let_sleep = sleep && has(toks, &["let", "lets", "allow", "allows"]) && !negated;
    let stop_from = sleep && has(toks, &["from"]);
    let stop_keeping = has(toks, &["stop", "quit", "end", "cancel", "release"])
        && (has(toks, &["keeping", "caffeinate", "caffeinating", "caffeinated"]) || has(toks, &["awake"]))
        && !stop_from;
    let_sleep || stop_keeping
}

/// ADR 0194: the task asks to keep the Mac (or its screen) awake: "keep my mac awake", "stay awake for 2 hours",
/// "caffeinate my mac", "don't let my mac sleep for an hour", "prevent the computer from sleeping", "stop my mac from
/// sleeping", "keep my screen on".
fn keep_awake_said(toks: &[Tok]) -> bool {
    let sleep = has(toks, SLEEP_WORDS);
    let keep = has(toks, &["keep", "keeps", "stay", "stays"]);
    let awake = keep && has(toks, &["awake"]);
    let caffeinate = has(toks, &["caffeinate"]);
    let prevent = sleep && has(toks, &["prevent", "don't", "dont", "not"]);
    let stop_from = sleep && has(toks, &["stop", "stops"]) && has(toks, &["from"]);
    let screen_on = keep && has(toks, DISPLAY_WORDS) && has(toks, &["on"]);
    awake || caffeinate || prevent || stop_from || screen_on
}

/// ADR 0194: "keep my mac awake for 2 hours", "every weekday at 9 keep the computer awake for 8 hours".
fn keep_awake_task(toks: &[Tok]) -> bool {
    keep_awake_said(toks) && !allow_sleep_said(toks) && !has(toks, KEEP_AWAKE_NOT_WORDS)
}

/// ADR 0194: "let my mac sleep again", "stop keeping my mac awake".
fn allow_sleep_task(toks: &[Tok]) -> bool {
    allow_sleep_said(toks) && !has(toks, KEEP_AWAKE_NOT_WORDS)
}

/// ADR 0194: the number a word before a time unit says ("2 hours", "an hour", "ninety minutes"), if any.
fn duration_count(w: &str) -> Option<u32> {
    Some(match w {
        "a" | "an" | "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        "fifteen" => 15,
        "twenty" => 20,
        "thirty" => 30,
        "forty" => 40,
        "forty-five" => 45,
        "fifty" => 50,
        "sixty" => 60,
        "ninety" => 90,
        d if !d.is_empty() && d.len() <= 4 && d.chars().all(|c| c.is_ascii_digit()) => d.parse().ok()?,
        _ => return None,
    })
}

/// ADR 0194: every duration the task says, in minutes: a count before hour(s)/hr(s)/minute(s)/min(s), "half an hour"
/// (30), "an hour and a half" (+30). A count after "every" / "each" is the schedule's cadence ("every 2 hours"), not a
/// duration. `None`: a duration ctrl won't guess (vague words with no number, 0, or more than 12 hours).
fn durations(toks: &[Tok]) -> Option<Vec<u32>> {
    let mut out: Vec<u32> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let unit = match t.w.as_str() {
            "hour" | "hours" | "hr" | "hrs" => 60,
            "minute" | "minutes" | "min" | "mins" => 1,
            _ => continue,
        };
        let w = |k: usize| i.checked_sub(k).map(|j| toks[j].w.as_str());
        if matches!(w(1), Some("every" | "each" | "per")) || matches!(w(2), Some("every" | "each")) {
            continue;
        }
        let Some(mut n) = w(1).and_then(duration_count).map(|c| c * unit) else {
            continue;
        };
        if unit == 60 && matches!(w(1), Some("an" | "a")) && w(2) == Some("half") {
            n = 30;
        }
        let next = |k: usize| toks.get(i + k).map(|x| x.w.as_str());
        if unit == 60 && next(1) == Some("and") && next(2) == Some("a") && next(3) == Some("half") {
            n += 30;
        }
        if n == 0 || n > crate::exec::MAX_AWAKE_MINUTES {
            return None;
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
    if out.is_empty() && has(toks, VAGUE_DURATION_WORDS) {
        return None;
    }
    Some(out)
}

/// ADR 0194: the keep-awake duration: the one the task says, else an hour; two different ones ask; one ctrl won't
/// guess offers nothing (the run asks).
fn duration_question(toks: &[Tok]) -> Question {
    let pick: Vec<(u32, i64)> = match durations(toks) {
        None => Vec::new(),
        Some(v) => match v.as_slice() {
            [] => vec![(crate::exec::DEFAULT_AWAKE_MINUTES, 6000)],
            [n] => vec![(*n, 6000)],
            many => many.iter().map(|n| (*n, 2500)).collect(),
        },
    };
    let cs = pick
        .into_iter()
        .map(|(n, p)| {
            choice(
                opt(format!("{}{n}", crate::exec::MINUTES_PREFIX), OptionKind::Scope),
                p,
            )
        })
        .collect();
    finish(Node::ChooseTarget, cs)
}

/// ADR 0183: "put my mac to sleep", "sleep my mac", "every night at 11 put the computer to sleep". A Mac word plus a
/// sleep word, and none of the words that mean another thing (the display alone is ADR 0180).
fn system_sleep_task(toks: &[Tok]) -> bool {
    has(toks, MAC_WORDS) && has(toks, &["sleep", "asleep"]) && !has(toks, SYSTEM_SLEEP_NOT_WORDS)
}

/// ADR 0180: "turn off my display", "put the screen to sleep", "sleep the displays", "monitor off". A display word plus
/// a sleep word or "off", and none of the words that mean another screen thing. "put my mac to sleep" (no display
/// word) is the whole Mac, not this, and is not built.
fn display_sleep_task(toks: &[Tok]) -> bool {
    has(toks, DISPLAY_WORDS)
        && has(toks, &["sleep", "sleeps", "asleep", "off"])
        && !has(toks, DISPLAY_NOT_WORDS)
        && !toks.windows(2).any(|w| w[0].w == "screen" && w[1].w == "time")
}

/// ADR 0176: the words that ask for an eject.
const EJECT_WORDS: &[&str] = &["eject", "ejects", "ejecting", "unmount", "unmounts", "unmounting"];
/// ADR 0176: words that make an eject task about every ejectable volume, not one. "every" counts only right before a
/// disk word ("eject every disk"): "every day eject the usb drive" is a schedule.
const EJECT_ALL_WORDS: &[&str] = &[
    "all",
    "everything",
    "both",
    "disks",
    "drives",
    "volumes",
    "sticks",
    "cards",
    "images",
    "dmgs",
    "ssds",
    "externals",
    "devices",
];
const EJECT_ONE_WORDS: &[&str] = &[
    "disk", "drive", "volume", "stick", "card", "image", "dmg", "ssd", "usb", "sd", "external", "device",
];

/// ADR 0176: whether an eject task means every ejectable volume ("eject my disks", "eject all drives", "eject
/// everything", "eject every disk") rather than the one disk it names ("eject the usb drive", "eject my sd card").
fn ejects_all(toks: &[Tok]) -> bool {
    has(toks, EJECT_ALL_WORDS)
        || toks
            .windows(2)
            .any(|w| w[0].w == "every" && (EJECT_ONE_WORDS.contains(&w[1].w.as_str())))
}

/// ADR 0187: the words that ask for a window to come back out of the Dock.
const UNMINIMIZE_WORDS: &[&str] = &[
    "unminimize",
    "unminimise",
    "un-minimize",
    "un-minimise",
    "unminimized",
    "unminimised",
    "deminimize",
    "deminimise",
    "deminiaturize",
];
/// ADR 0187: the words for minimizing (or a window that is minimized).
const MINIMIZE_WORDS: &[&str] = &[
    "minimize",
    "minimise",
    "minimizes",
    "minimises",
    "minimized",
    "minimised",
    "minimizing",
    "minimising",
];
/// ADR 0187: words that turn a minimized window into one that comes back ("show slack's minimized windows", "restore
/// my minimized window").
const UNMINIMIZE_BACK_WORDS: &[&str] = &[
    "restore", "restores", "bring", "back", "show", "get", "open", "reopen", "unhide", "return",
];
/// ADR 0187: the Dock's minimize settings ("minimize windows into the app icon", "the genie effect") are settings,
/// not a window task: neither intent.
const MINIMIZE_SETTING_WORDS: &[&str] = &[
    "effect",
    "genie",
    "scale",
    "animation",
    "setting",
    "settings",
    "preference",
    "preferences",
    "icon",
    "double-click",
    "double-clicking",
];

/// ADR 0187: "unminimize slack", "restore slack windows", "show my minimized windows", "bring slack back from the
/// dock", "get my windows out of the dock".
fn unminimize_task(toks: &[Tok]) -> bool {
    if has(toks, MINIMIZE_SETTING_WORDS) {
        return false;
    }
    let windows = has(toks, &["window", "windows"]);
    has(toks, UNMINIMIZE_WORDS)
        || (has(toks, MINIMIZE_WORDS) && has(toks, UNMINIMIZE_BACK_WORDS))
        || (has(toks, &["restore", "restores"]) && windows)
        || (has(toks, &["dock"])
            && has(toks, &["bring", "get", "take", "pull"])
            && has(toks, &["back", "out", "from"]))
}

/// ADR 0187: "minimize slack", "minimise all my windows", "every evening minimize textedit". Not one that comes back.
fn minimize_task(toks: &[Tok]) -> bool {
    has(toks, MINIMIZE_WORDS) && !has(toks, MINIMIZE_SETTING_WORDS) && !unminimize_task(toks)
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
            if named_app && (!site || full_named) && !menu_bar_wording(toks) {
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
        "hide_app" => {
            // ADR 0149: "hide slack", "hide spotify every evening": "hide" plus one named app and no group word.
            // Without both it sits well below the rest, like hide_others. (Before this, "hide slack" scored as
            // open_app on the app name alone.)
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
                ],
            );
            // ADR 0173: "hide the dock" is the Dock's auto-hide, not hiding the Dock app.
            // ADR 0197: nor is "hide the menu bar" (or "hide the dock and the menu bar") a hide of an app.
            if has(toks, &["hide"])
                && named_app
                && !group
                && !dock_task(toks)
                && !dock_edge_task(toks)
                && !menu_bar_named(toks)
            {
                p += 9000;
            }
        }
        "show_app" => {
            // ADR 0152: "show slack", "unhide spotify": "show"/"unhide" plus one named app, no group word and no site
            // ("show me nytimes.com" stays open_url). Before this, "show slack" scored as open_app (a hidden launch
            // of a running app: nothing came back).
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
                ],
            );
            // "show me my calendar" / "show my calendar" mean open it (ADR 0138's calendar), so "show" counts only
            // without "me" / "my"; "unhide" always does.
            let asks = has(toks, &["unhide"]) || (has(toks, &["show"]) && !has(toks, &["me", "my"]));
            // ADR 0173: "show the dock" turns its auto-hide off, it doesn't unhide the Dock app.
            if asks
                && named_app
                && !group
                && (!site || full_named)
                && !dock_task(toks)
                && !dock_edge_task(toks)
                && !menu_bar_named(toks)
            {
                p += 9000;
            }
        }
        // ADR 0187: above show_app ("show slack minimized windows" is not an unhide) and close_windows (a window word
        // alone). Without the words both sit well below the rest, like quit_unused.
        "unminimize_app" => {
            p -= 3000;
            // "restore slack": one named app and "restore" (no window word) is its windows coming back too, never a
            // hidden launch that reports done.
            let restore_app =
                has(toks, &["restore"]) && named_app && !site && !has(toks, MINIMIZE_SETTING_WORDS);
            if unminimize_task(toks) || restore_app {
                p += 12000;
            }
        }
        "minimize_app" => {
            p -= 3000;
            if minimize_task(toks) {
                p += 12000;
            }
        }
        "focus_mode" => {
            // ADR 0155: "turn on do not disturb", "dnd off", "turn on focus", "start a work focus". No app named (that's
            // one app's own setting), no site.
            p -= 3000;
            let dnd = has(toks, &["disturb", "dnd"]);
            let focus = has(toks, &["focus"])
                && has(
                    toks,
                    &[
                        "on", "off", "turn", "enable", "disable", "start", "stop", "end", "mode", "set",
                        "toggle",
                    ],
                );
            if (dnd || focus) && !named_app && !site {
                p += 9500;
            }
        }
        "open_folder" => {
            // ADR 0166: "open my downloads", "open the documents folder", "show my desktop folder": an open word plus
            // one of the user's folders, no tidy word (that's clean_folder), no site and no app other than the
            // Finder. "show" counts only with "my" / "folder": "show the desktop" is macOS's Show Desktop (windows
            // aside), not a Finder window. Before this, "open my downloads" scored as clean_folder and asked.
            p -= 3000;
            // Run 40 (ADR 0166 amendment 1): Pictures and Movies count with "my" or "folder" ("open my pictures",
            // "open the movies folder"); Music only with "folder", since "open music" / "open my music" is the Music
            // app. When the Music folder is what's named, the Music app named by that same word isn't another app.
            let music_folder = has(toks, &["music"]) && has(toks, &["folder"]);
            let folder = has(toks, USER_FOLDER_WORDS)
                || (has(toks, MEDIA_FOLDER_WORDS) && has(toks, &["my", "folder"]))
                || music_folder;
            let opens = has(toks, &["open", "browse", "view"])
                || (has(toks, &["show", "go"]) && has(toks, &["my", "folder"]));
            let tidy = has(
                toks,
                &[
                    "clean",
                    "tidy",
                    "organize",
                    "organise",
                    "sort",
                    "declutter",
                    "clear",
                    "delete",
                    "trash",
                    "remove",
                    "move",
                ],
            );
            let other_app = named
                .iter()
                .any(|(a, _)| a.id != crate::ladder::FINDER && !(music_folder && a.id == MUSIC_APP));
            if folder && opens && !tidy && !other_app && !site {
                p += 9000;
            }
        }
        "clear_clipboard" => {
            // ADR 0168: "clear my clipboard", "empty the clipboard", "wipe what i copied". A clipboard word plus a
            // clearing word, no app named ("clear slack's clipboard" isn't a thing ctrl knows) and no site. Without
            // both it sits well below the rest.
            p -= 3000;
            let clip = has(toks, &["clipboard", "pasteboard"])
                || (has(toks, &["copied"]) && has(toks, &["what", "whatever", "everything", "i"]));
            let clears = has(
                toks,
                &[
                    "clear", "empty", "wipe", "erase", "delete", "remove", "reset", "flush", "purge",
                ],
            );
            if clip && clears && !named_app && !site {
                p += 9500;
            }
        }
        "eject_disks" => {
            // ADR 0176: "eject my disks", "unmount the usb drive", "every day at 6pm eject all drives". An eject word,
            // no app named but the Finder ("eject my disks in finder"), no site. Without it it sits well below.
            p -= 3000;
            let other_app = named.iter().any(|(a, _)| a.id != crate::ladder::FINDER);
            if has(toks, EJECT_WORDS) && !other_app && !site {
                p += 9500;
            }
        }
        "display_sleep" => {
            // ADR 0180: "turn off my display", "put the screen to sleep", "every night at 11 sleep the displays". No
            // app named in full ("turn off activity monitor" is that app), no site. Without it it sits well below.
            p -= 3000;
            let full = named.iter().any(|(a, h)| *h as usize == a.tokens.len());
            if display_sleep_task(toks) && !full && !site {
                p += 9500;
            }
        }
        "system_sleep" => {
            // ADR 0183: "put my mac to sleep", "every night at 11 sleep my mac". No app named in full, no site.
            // Without it it sits well below.
            p -= 3000;
            let full = named.iter().any(|(a, h)| *h as usize == a.tokens.len());
            if system_sleep_task(toks) && !full && !site {
                p += 9500;
            }
        }
        "wifi_power" => {
            // ADR 0191: "turn off my wifi", "turn wi-fi back on". No app named in full, no site. Without it it sits
            // well below.
            p -= 3000;
            let full = named.iter().any(|(a, h)| *h as usize == a.tokens.len());
            if wifi_task(toks) && !full && !site {
                p += 9500;
            }
        }
        "keep_awake" | "allow_sleep" => {
            // ADR 0194: "keep my mac awake for 2 hours", "let my mac sleep again". No app named in full, no site.
            // Without it it sits well below.
            p -= 3000;
            let full = named.iter().any(|(a, h)| *h as usize == a.tokens.len());
            let task = if id == "keep_awake" {
                keep_awake_task(toks)
            } else {
                allow_sleep_task(toks)
            };
            if task && !full && !site {
                p += 9500;
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
        "dock_autohide" => {
            // ADR 0173: "hide the dock", "auto-hide the dock", "always show the dock", "stop hiding the dock". The
            // Dock plus a hide / show / on-off word, no other app named, no site. Without it it sits well below.
            p -= 3000;
            let other_app = named.iter().any(|(a, _)| a.id != DOCK_APP);
            if dock_task(toks) && !other_app && !site {
                p += 9500;
            }
        }
        "dock_edge" => {
            // ADR 0195: "move the dock to the left", "put the dock on the right", "dock on the bottom". The Dock
            // plus an edge word, no hide / show / autohide (those stay dock_autohide), no other app named, no site.
            p -= 3000;
            let other_app = named.iter().any(|(a, _)| a.id != DOCK_APP);
            if dock_edge_task(toks) && !other_app && !site {
                p += 9500;
            }
        }
        "menu_bar_autohide" => {
            // ADR 0197: "hide the menu bar", "always show the menu bar", "stop hiding the menu bar". The menu bar plus
            // a hide / show / on-off word, no Dock, no app named, no site. Without it it sits well below.
            p -= 3000;
            if menu_bar_task(toks) && !named_app && !site {
                p += 9500;
            }
        }
        "screensaver" => {
            // ADR 0178: "start the screensaver" / "start screensaver" / "turn on the screen saver", and (amendment 1)
            // "stop the screensaver". No named app, no site, not its setting or a lock. Without it it sits well below.
            p -= 3000;
            if screensaver_task(toks) && !named_app && !site {
                p += 9500;
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

/// ADR 0166: which of the user's own folders to show. Only the closed set (no screenshots: that's a place on the
/// Desktop, not a folder of its own here); nothing named, nothing preferred, so the chain asks.
fn user_folder_question(toks: &[Tok]) -> Question {
    let folders: [(crate::ladder::UserFolder, &[&str]); 6] = [
        (crate::ladder::UserFolder::Downloads, &["downloads", "download"]),
        (crate::ladder::UserFolder::Desktop, &["desktop"]),
        (crate::ladder::UserFolder::Documents, &["documents", "docs"]),
        (crate::ladder::UserFolder::Pictures, &["pictures", "pics"]),
        (crate::ladder::UserFolder::Music, &["music"]),
        (crate::ladder::UserFolder::Movies, &["movies", "videos"]),
    ];
    let cs = folders
        .iter()
        .map(|(f, words)| {
            choice(
                opt(format!("folder.{}", f.as_str()), OptionKind::Folder),
                5000 * count(toks, words).min(1),
            )
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
        // ADR 0148: one named app quits through the workspace (`terminate()`, the Dock's Quit), never a key event;
        // the ladder refuses the frontmost app, and there's no fallback move to backtrack into.
        "quit_app" => vec![irreversible(route("workspace.quit_app", 4500))],
        "quit_unused" => vec![irreversible(route("workspace.quit_unused", 4500))],
        "hide_others" => vec![route("workspace.hide_others", 4500)],
        // ADR 0149: `NSRunningApplication.hide()` of the one named app; no key event (⌘H would go to the app the user
        // is in), so nothing to backtrack into.
        "hide_app" => vec![route("workspace.hide_app", 4500)],
        // ADR 0152: `NSRunningApplication.unhide()` of the one named app; no activation, no key event.
        "show_app" => vec![route("workspace.show_app", 4500)],
        // ADR 0187: one move each, a hold: asked out, nothing read or written, until the Mac probe shows an AXMinimized
        // write leaves the user's front window, app, focus and cursor alone.
        "unminimize_app" => vec![route("windows.unminimize_hold", 4500)],
        "minimize_app" => vec![route("windows.minimize_hold", 4500)],
        // ADR 0166: the folder in a Finder window through LaunchServices (`activates = false`); no key event, no
        // AX action, so nothing to backtrack into.
        "open_folder" => vec![route("workspace.open_folder", 4500)],
        // ADR 0155: no background route to set a Focus (no public API; the Shortcuts CLI needs a shortcut the user
        // makes and a process ctrl doesn't spawn yet), so the one move is a hold: asked out, nothing touched.
        // ADR 0158: the approved route, the user's one "qrtz Set Focus" shortcut through Shortcuts Events, with the
        // mode from a closed set (never the task's words themselves). Off when the task says so.
        "focus_mode" => {
            let off = has(toks, &["off", "disable", "stop", "end"]);
            let mode = if off {
                crate::ladder::FocusMode::Off
            } else {
                focus_mode_of(toks)
            };
            vec![route(&format!("system.focus.{}", mode.as_str()), 4500)]
        }
        // ADR 0168: `NSPasteboard.general.clearContents()` in the helper's own process; no key event, nothing to
        // backtrack into.
        "clear_clipboard" => vec![route("system.clear_clipboard", 4500)],
        // ADR 0176: one move only, from the words: a singular task that fails must never backtrack into ejecting
        // every disk.
        "eject_disks" => vec![route(
            if ejects_all(toks) {
                "system.eject_all"
            } else {
                "system.eject_one"
            },
            4500,
        )],
        // ADR 0180: one move — the displays sleep.
        "display_sleep" => vec![route("system.display_sleep", 4500)],
        // ADR 0183: one move — the whole Mac sleeps.
        "system_sleep" => vec![route("system.system_sleep", 4500)],
        // ADR 0191: one move, the direction the words say: a failed "off" never backtracks into turning it on.
        "wifi_power" => vec![route(
            if wifi_dir(toks) == Some(true) {
                "system.wifi_on"
            } else {
                "system.wifi_off"
            },
            4500,
        )],
        // ADR 0194: one move; a display word keeps the screen on too.
        "keep_awake" => vec![route(
            if has(toks, DISPLAY_WORDS) {
                "system.keep_awake_display"
            } else {
                "system.keep_awake"
            },
            4500,
        )],
        "allow_sleep" => vec![route("system.allow_sleep", 4500)],
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
        // ADR 0137: on or off from the words ("light mode", "turn off dark mode" → off).
        // ADR 0145: "toggle" / "flip" (with no on or off said) flips whatever the appearance is when it runs.
        "dark_mode" => {
            let flip = has(toks, &["toggle", "flip"]);
            let off = !flip && has(toks, &["light", "off", "disable"]);
            let on = !flip && !off && has(toks, &["dark", "on", "enable"]);
            let mut v = vec![
                route("scripting.dark_mode_on", if on { 4500 } else { 2000 }),
                route("scripting.dark_mode_off", if off { 4500 } else { 2000 }),
            ];
            if flip {
                v.push(route("scripting.dark_mode_toggle", 4500));
            }
            v
        }
        // ADR 0173: hidden or shown from the words (see `dock_hides`).
        "dock_autohide" => {
            let hide = dock_hides(toks);
            vec![
                route("scripting.dock_autohide_on", if hide { 4500 } else { 2000 }),
                route("scripting.dock_autohide_off", if hide { 2000 } else { 4500 }),
            ]
        }
        // ADR 0197: one move from the words (see `dock_hides`, the same words): hidden or always shown, never both.
        "menu_bar_autohide" => {
            if dock_hides(toks) {
                vec![route("scripting.menu_bar_autohide_on", 4500)]
            } else {
                vec![route("scripting.menu_bar_autohide_off", 4500)]
            }
        }
        // ADR 0195: one move from the words (left / right / bottom). Ambiguous → all three low so the run asks.
        "dock_edge" => match dock_edge_of(toks) {
            Some("left") => vec![route("scripting.dock_edge_left", 4500)],
            Some("right") => vec![route("scripting.dock_edge_right", 4500)],
            Some("bottom") => vec![route("scripting.dock_edge_bottom", 4500)],
            _ => vec![
                route("scripting.dock_edge_left", 2000),
                route("scripting.dock_edge_right", 2000),
                route("scripting.dock_edge_bottom", 2000),
            ],
        },
        // ADR 0178: one move — start the current screen saver, or (amendment 1) stop it. Never both: a stop task
        // never backtracks into starting it.
        "screensaver" => {
            if screensaver_stops(toks) {
                vec![route("scripting.screensaver_stop", 4500)]
            } else {
                vec![route("scripting.screensaver", 4500)]
            }
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
        Slot::Folder | Slot::TabScope | Slot::Level | Slot::Step | Slot::UserFolder | Slot::Duration => {
            Some(Node::ChooseTarget)
        }
        Slot::Url | Slot::SearchText => Some(Node::ChooseText),
    };
    if let Some(n) = slot_node {
        if !chosen.contains_key(&n) {
            return Next::Ask(match def.slot {
                Slot::Folder => folder_question(&toks),
                Slot::UserFolder => user_folder_question(&toks),
                Slot::TabScope => scope_question(&toks),
                Slot::Url => url_question(goal, &toks),
                Slot::SearchText => search_question(goal, &toks),
                Slot::Level => level_question(&toks),
                Slot::Step => step_question(&toks),
                Slot::Duration => duration_question(&toks),
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
        "hide_app" => "hide an app",
        "show_app" => "show a hidden app",
        "unminimize_app" => "bring back minimized windows",
        "minimize_app" => "minimize windows",
        "open_folder" => "open a folder",
        "focus_mode" => "turn focus or do not disturb on or off",
        "clear_clipboard" => "clear your clipboard",
        "eject_disks" => "eject your disks",
        "display_sleep" => "turn off your display",
        "system_sleep" => "put your mac to sleep",
        "wifi_power" => "turn wi-fi on or off",
        "keep_awake" => "keep your mac awake for a while",
        "allow_sleep" => "let your mac sleep again",
        "mute" => "mute your mac",
        "unmute" => "unmute your mac",
        "set_volume" => "set the volume",
        "change_volume" => "turn the volume up or down",
        "empty_trash" => "empty the trash",
        "dark_mode" => "switch dark mode",
        "dock_autohide" => "hide or show the dock",
        "dock_edge" => "move the dock to an edge",
        "menu_bar_autohide" => "hide or show the menu bar",
        "screensaver" => "start or stop the screensaver",
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
        "workspace.quit_app" => "quit it without bringing it forward",
        "workspace.hide_others" => "hide them, the app you're in stays",
        "workspace.hide_app" => "hide it, nothing comes forward",
        "workspace.show_app" => "show its windows, the app you're in stays in front",
        "workspace.open_folder" => "show it in a finder window, nothing comes forward",
        "system.focus_hold" => "leave focus as it is, not supported yet",
        "windows.unminimize_hold" => "leave them in the dock, not checked on a mac yet",
        "windows.minimize_hold" => "leave the windows as they are, not checked on a mac yet",
        "system.focus.off" => "turn focus off",
        "system.focus.do_not_disturb" => "turn on do not disturb",
        m if m.starts_with("system.focus.") => "turn on that focus",
        "system.clear_clipboard" => "empty the clipboard, nothing read from it",
        "system.eject_all" => "eject every external disk, none forced",
        "system.display_sleep" => "put the displays to sleep, the mac stays awake",
        "system.system_sleep" => "put the whole mac to sleep, nothing closed",
        "system.wifi_on" => "turn the wi-fi radio on, no network picked",
        "system.wifi_off" => "turn the wi-fi radio off, nothing else changed",
        "system.eject_one" => "eject the one external disk, never forced",
        "system.keep_awake" => "keep the mac from sleeping, the screen may still sleep",
        "system.keep_awake_display" => "keep the mac and its screen from sleeping",
        "system.allow_sleep" => "stop keeping it awake, nothing put to sleep",
        "system.mute" => "mute the sound output",
        "system.unmute" => "turn the sound output back on",
        "system.set_volume" => "set the sound output to that level",
        "system.volume_up" => "turn the sound output up a step",
        "system.volume_down" => "turn the sound output down a step",
        "scripting.dark_mode_on" => "switch to dark mode",
        "scripting.dark_mode_off" => "switch to light mode",
        "scripting.dark_mode_toggle" => "flip the appearance",
        "scripting.dock_autohide_on" => "hide the dock until the pointer reaches it",
        "scripting.dock_autohide_off" => "always show the dock",
        "scripting.dock_edge_left" => "put the dock on the left",
        "scripting.dock_edge_right" => "put the dock on the right",
        "scripting.dock_edge_bottom" => "put the dock on the bottom",
        "scripting.menu_bar_autohide_on" => "hide the menu bar until the pointer reaches it",
        "scripting.menu_bar_autohide_off" => "always show the menu bar, full screen as set",
        "scripting.screensaver" => "start the current screen saver",
        "scripting.screensaver_stop" => "stop the screen saver, nothing unlocked",
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
        // ADR 0194: a keep-awake duration in minutes, said in whole hours when it is some.
        "minutes" => match rest.parse::<u32>() {
            Ok(60) => "for an hour".into(),
            Ok(n) if n % 60 == 0 => format!("for {} hours", n / 60),
            Ok(1) => "for a minute".into(),
            _ => format!("for {rest} minutes"),
        },
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

/// ADR 0158: the Focus a task names, from a closed list of words; do not disturb when it names none.
pub(crate) fn focus_mode_of(toks: &[Tok]) -> crate::ladder::FocusMode {
    use crate::ladder::FocusMode as F;
    let table: [(&[&str], F); 8] = [
        (&["work", "working"], F::Work),
        (&["personal"], F::Personal),
        (&["sleep", "sleeping", "bedtime"], F::Sleep),
        (&["driving", "drive"], F::Driving),
        (&["fitness", "workout"], F::Fitness),
        (&["gaming", "game"], F::Gaming),
        (&["mindfulness", "mindful"], F::Mindfulness),
        (&["reading", "read"], F::Reading),
    ];
    table
        .iter()
        .find(|(w, _)| has(toks, w))
        .map(|(_, m)| *m)
        .unwrap_or(F::DoNotDisturb)
}

// Run 49: the labels are what the decision layer reads. Laya scores an option by its text (CtrlLayaSequence encodes
// each option's label after its `<mask>`), so an option that falls through to a generic label, or two options of one
// question with the same text, can't be told apart by the model; and the model has 32 option slots.
#[cfg(test)]
mod label_tests {
    use super::*;

    #[test]
    fn every_intent_has_its_own_label() {
        let mut seen = BTreeMap::new();
        for d in INTENTS {
            let l = intent_label(d.id);
            assert_ne!(l, "something else", "{} has no label", d.id);
            assert_eq!(l, l.to_lowercase(), "{}: ui text is lowercase", d.id);
            if let Some(other) = seen.insert(l, d.id) {
                // Two intents may share a label only when one task reads as either (none do today).
                panic!("{} and {other} share the label {l:?}", d.id);
            }
        }
    }

    #[test]
    fn every_offered_move_has_a_label_and_no_question_repeats_one() {
        let phrasings = [
            "",
            "on",
            "off",
            "turn on",
            "turn off",
            "up",
            "down",
            "stop",
            "toggle",
            "all",
            "one",
            "delete",
            "sort",
            "old",
            "light",
            "dark",
            "hide",
            "show",
            "wifi on",
            "wifi off",
            "clean",
            "trash",
            "screen awake",
            "left",
            "right",
            "bottom",
            "menu bar",
            "always show the menu bar",
        ];
        for d in INTENTS {
            for p in phrasings {
                let toks = tokens(p);
                let q = move_question(d.id, &toks, &BTreeMap::new(), None);
                assert!(q.choices.len() <= MAX_OPTIONS, "{} {p:?}", d.id);
                let mut labels = BTreeMap::new();
                for c in &q.choices {
                    let l = label(&c.opt, None);
                    if c.opt.id.starts_with("route.") {
                        assert_ne!(l, "a route", "{} {p:?}: {} has no label", d.id, c.opt.id);
                    }
                    assert_eq!(l, l.to_lowercase(), "{}: ui text is lowercase", c.opt.id);
                    if let Some(other) = labels.insert(l.clone(), c.opt.id.clone()) {
                        panic!("{} {p:?}: {} and {other} both read {l:?}", d.id, c.opt.id);
                    }
                }
            }
        }
    }

    #[test]
    fn a_question_fits_the_models_option_slots() {
        // laya_multilingual_e8_L512_options32: marker_map [1, 32, L], probabilities [1, 32] (CtrlLayaSequence.maxOptions).
        const LAYA_OPTION_SLOTS: usize = 32;
        const { assert!(MAX_OPTIONS <= LAYA_OPTION_SLOTS) };
        // There are more intents than a question holds (30 > 19 + "none of these"): `finish` keeps the best by prior,
        // so the intent question never overflows the slots however many intents are added.
        let q = finish(
            Node::ChooseIntent,
            INTENTS
                .iter()
                .map(|d| choice(opt(format!("intent.{}", d.id), OptionKind::Intent), 0))
                .collect(),
        );
        assert_eq!(q.choices.len(), MAX_OPTIONS);
        assert!(INTENTS.len() > MAX_OPTIONS - 1);
    }
}

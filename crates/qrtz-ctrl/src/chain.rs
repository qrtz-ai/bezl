//! The chain's option sources and heuristic priors (ADR 0214; decision-layer spec §2–§5). [`next`] is a pure
//! function of the goal and the folded state: it says what the run needs next (an observation, a question, an
//! action, a backtrack or the end). The driver performs it and appends the result; replay calls the same function
//! on the same log and must get the same questions ([`crate::driver::replay_check`]).
//!
//! The chain: `choose_app` (open it) → `choose_move` (menu-bar shortcut, built-in shortcut, open the address, or a
//! top-level branch) → `choose_element` (descend into a branch or a cluster, or an element action) → act → read the
//! tree → `choose_text` when a text field just got focus, else `verify` (done / keep going / it didn't work).
//!
//! Priors are milli-logits: overlap of the goal's words with an option's hashed tokens (content words count more
//! than verbs), the default browser for an address, the address bar for a site, a search field for a search.
//! "none of these" is always last at [`NONE_PRIOR`], so a vague goal asks instead of guessing.
use crate::act::{combo_allowed, Action, BUILTIN_SHORTCUTS};
use crate::ax::{
    entries, find, label_tokens, level, role_slug, subtree_tokens, words, AxNode, Entry, Level, MAX_ENTRIES,
};
use crate::decide::{BacktrackReason, Choice, Goal, Node, Opt, OptionKind, Question, NONE_PRIOR};
use crate::gate::COMBO_RULES;
use crate::log::{ActionStatus, CtrlState, Phase, TaskStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// At most this many actions per run (the step budget), and decisions.
pub const MAX_ACTIONS: usize = 8;
pub const MAX_DECISIONS: u64 = 32;
/// Roles whose press sets a state ("turn on wi-fi").
pub const TOGGLE_ROLES: &[&str] = &["AXCheckBox", "AXSwitch", "AXRadioButton"];
/// Menu shortcuts the move question keeps (the best by prior, only those that match the goal).
const MENU_KEEP: usize = 8;

// ---------------------------------------------------------------------------------------------------------------
// The app catalog

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppObs {
    pub id: String,
    pub tokens: Vec<String>,
    /// The name's words as written (hashed), without singulars.
    #[serde(default)]
    pub names: Vec<String>,
    pub browser: bool,
    pub running: bool,
}

/// What `obs.apps` records: the catalog the chain chose from (bundle ids, hashed name tokens).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppsObs {
    pub apps: Vec<AppObs>,
    pub default_browser: Option<String>,
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
}

impl AppCatalog {
    /// The log form: names become hashed tokens, sorted by id.
    pub fn observe(&self) -> AppsObs {
        let mut apps: Vec<AppObs> = self
            .apps
            .iter()
            .filter(|a| crate::ax::valid_id(&a.id.replace('.', "_")))
            .map(|a| AppObs {
                id: a.id.clone(),
                tokens: label_tokens(&a.name),
                names: words(&crate::gate::fold_label(&a.name))
                    .into_iter()
                    .map(|(w, _, _)| crate::ax::tok_hash(&w))
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
        }
    }
}

/// Readable labels for the UI and the Laya prompt (memory only): element handle or bundle id → title.
pub trait Labels {
    fn get(&self, key: &str) -> Option<String>;
}
impl Labels for BTreeMap<String, String> {
    fn get(&self, key: &str) -> Option<String> {
        BTreeMap::get(self, key).cloned()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The goal

const STOP: &[&str] = &[
    "a", "an", "the", "my", "to", "in", "of", "for", "on", "at", "it", "me", "please", "and", "then", "with",
    "ctrl", "can", "you", "i", "want", "is", "that", "this", "app", "into", "from", "by",
];
const VERBS: &[&str] = &[
    "open", "go", "launch", "start", "new", "create", "make", "add", "click", "press", "tap", "type",
    "write", "search", "find", "look", "up", "set", "turn", "toggle", "show", "called", "titled", "named",
    "visit", "switch", "enter", "put", "enable", "disable",
];
/// Sites a bare name means (`open youtube` → `youtube.com`).
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
    "x",
];
const SITE_ALIASES: &[(&str, &str)] = &[("gmail", "mail.google.com"), ("x", "x.com")];

/// A text candidate for `choose_text` (and an address for `open url`): a pointer into the goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextCand {
    pub label_ref: String,
    pub kind: TextKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// A literal address in the goal ("apple.com").
    Domain,
    /// A known site's name ("youtube" → youtube.com).
    Site,
    /// What to search for.
    Search,
    /// Quoted, or after "titled / called / named / type / write".
    Literal,
}

/// The goal as the chain reads it: hashed content words and verbs, and the text candidates.
#[derive(Debug, Clone, Default)]
pub struct GoalInfo {
    pub targets: Vec<String>,
    pub verbs: BTreeSet<String>,
    pub cands: Vec<TextCand>,
    pub wants_search: bool,
    pub wants_new: bool,
    pub wants_tab: bool,
    pub wants_settings: bool,
    /// "turn on", "toggle", "enable": the goal is a switch's state.
    pub wants_toggle: bool,
}

fn is_domain(w: &str) -> bool {
    match w.rsplit_once('.') {
        Some((head, tld)) => {
            !head.is_empty()
                && !head.starts_with('.')
                && (2..=6).contains(&tld.len())
                && tld.chars().all(|c| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// The span after the first of `cue` words to the end of the goal (trailing "in <app>" dropped).
fn span_after(ws: &[(String, usize, usize)], cue: &[&str], skip: &[&str]) -> Option<(usize, usize)> {
    let mut i = ws.iter().position(|(w, _, _)| cue.contains(&w.as_str()))?;
    if let Some(f) = ws[i..].iter().position(|(w, _, _)| w == "for") {
        i += f;
    }
    let mut j = i + 1;
    while j < ws.len() && skip.contains(&ws[j].0.as_str()) {
        j += 1;
    }
    let s = ws.get(j)?.1;
    let mut end_w = ws.len();
    if let Some(k) = ws[j..]
        .iter()
        .position(|(w, _, _)| w == "in" || w == "on")
        .map(|k| k + j)
    {
        if k > j {
            end_w = k;
        }
    }
    let e = ws[end_w - 1].2;
    (e > s).then_some((s, e))
}

impl GoalInfo {
    pub fn read(goal: &str) -> GoalInfo {
        let ws = words(goal);
        let mut g = GoalInfo::default();
        for (w, _, _) in &ws {
            if VERBS.contains(&w.as_str()) {
                g.verbs.extend(label_tokens(w));
            } else if !STOP.contains(&w.as_str()) {
                for t in label_tokens(w) {
                    if !g.targets.contains(&t) {
                        g.targets.push(t);
                    }
                }
            }
        }
        let has = |x: &[&str]| ws.iter().any(|(w, _, _)| x.contains(&w.as_str()));
        g.wants_search = has(&["search", "find", "look"]);
        g.wants_new = has(&["new", "create", "make", "add"]);
        g.wants_tab = has(&["tab"]);
        g.wants_settings = has(&["settings", "preferences"]);
        g.wants_toggle = has(&["turn", "toggle", "enable", "disable", "switch"]);
        // Quoted text.
        let mut quoted = false;
        for (q0, q1) in [('"', '"'), ('“', '”'), ('\'', '\'')] {
            if let Some(a) = goal.find(q0) {
                let s = a + q0.len_utf8();
                if let Some(b) = goal[s..].find(q1) {
                    if b > 0 {
                        g.cands.push(TextCand {
                            label_ref: format!("goal#{s}..{}", s + b),
                            kind: TextKind::Literal,
                        });
                        quoted = true;
                        break;
                    }
                }
            }
        }
        // Literal addresses: whitespace-separated pieces that look like a domain.
        let mut off = 0;
        for piece in goal.split_whitespace() {
            let at = goal[off..].find(piece).map_or(off, |i| i + off);
            off = at + piece.len();
            let t = piece.trim_matches(|c: char| !c.is_alphanumeric() && c != '/');
            let lead = piece.find(t).unwrap_or(0);
            let host = t.split('/').next().unwrap_or(t);
            if is_domain(host) || t.starts_with("http://") || t.starts_with("https://") {
                g.cands.push(TextCand {
                    label_ref: format!("goal#{}..{}", at + lead, at + lead + t.len()),
                    kind: TextKind::Domain,
                });
            }
        }
        if !g.cands.iter().any(|c| c.kind == TextKind::Domain) {
            if let Some((_, s, e)) = ws.iter().find(|(w, _, _)| KNOWN_SITES.contains(&w.as_str())) {
                g.cands.push(TextCand {
                    label_ref: format!("site:goal#{s}..{e}"),
                    kind: TextKind::Site,
                });
            }
        }
        if !quoted {
            if let Some((s, e)) = span_after(&ws, &["search", "find", "look"], &["for", "up", "the", "a"]) {
                g.cands.push(TextCand {
                    label_ref: format!("goal#{s}..{e}"),
                    kind: TextKind::Search,
                });
            }
            if let Some((s, e)) = span_after(
                &ws,
                &["titled", "called", "named", "type", "write"],
                &["the", "a"],
            ) {
                g.cands.push(TextCand {
                    label_ref: format!("goal#{s}..{e}"),
                    kind: TextKind::Literal,
                });
            }
        }
        g
    }

    fn overlap(&self, toks: &BTreeSet<String>) -> (i64, i64) {
        let t = self.targets.iter().filter(|x| toks.contains(*x)).count() as i64;
        let v = self.verbs.iter().filter(|x| toks.contains(*x)).count() as i64;
        (t, v)
    }

    /// A gated option the goal doesn't name ("quit notes" when the goal is a new note) is a poor guess: its prior
    /// drops so the chain asks or tries something else first. The gate itself is unaffected.
    fn risk_penalty(&self, risk: &[String]) -> i64 {
        if risk.is_empty() {
            return 0;
        }
        let named = risk.iter().any(|r| {
            r.strip_prefix("label:").is_some_and(|w| {
                label_tokens(w)
                    .iter()
                    .any(|t| self.targets.contains(t) || self.verbs.contains(t))
            })
        });
        if named {
            0
        } else {
            -4000
        }
    }

    fn has_address(&self) -> bool {
        self.cands
            .iter()
            .any(|c| matches!(c.kind, TextKind::Domain | TextKind::Site))
    }
}

/// Resolve a text pointer against the goal: `goal#s..e` is that span; `site:goal#s..e` is the site it names.
pub fn resolve(label_ref: &str, goal: &str) -> Option<String> {
    let (site, r) = match label_ref.strip_prefix("site:") {
        Some(r) => (true, r),
        None => (false, label_ref),
    };
    let (s, e) = r.strip_prefix("goal#")?.split_once("..")?;
    let (s, e): (usize, usize) = (s.parse().ok()?, e.parse().ok()?);
    let t = goal.get(s..e)?.trim();
    if t.is_empty() {
        return None;
    }
    if !site {
        return Some(t.to_string());
    }
    let w = t.to_lowercase();
    Some(
        SITE_ALIASES
            .iter()
            .find(|(k, _)| *k == w)
            .map_or_else(|| format!("{w}.com"), |(_, v)| v.to_string()),
    )
}

// ---------------------------------------------------------------------------------------------------------------
// What comes next

#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    Catalog,
    /// Read `app`'s front window (`scope: None`) or one element's children.
    Tree {
        app: String,
        scope: Option<String>,
    },
    Shortcuts {
        app: String,
    },
    Ask(Question),
    Act(Action),
    Backtrack(BacktrackReason),
    Finish(TaskStatus),
    /// Asking Jared, waiting for a confirm, or finished: nothing to do.
    Wait,
}

/// The run's position, part of every question's input hash.
pub fn position(s: &CtrlState) -> String {
    format!(
        "{}|{}|{}",
        serde_json::to_string(&s.phase).unwrap_or_default(),
        s.tree.as_ref().map_or("-", |t| t.hash.as_str()),
        s.app.as_deref().unwrap_or("-")
    )
}

fn app_is_browser(s: &CtrlState, app: &str) -> bool {
    s.apps
        .as_ref()
        .is_some_and(|a| a.apps.iter().any(|x| x.id == app && x.browser))
}

pub fn next(goal: &Goal, s: &CtrlState, labels: &dyn Labels) -> Next {
    if s.status.is_some() || s.ask.is_some() || s.confirm.as_ref().is_some_and(|g| g.answer.is_none()) {
        return Next::Wait;
    }
    if s.decisions_made >= MAX_DECISIONS && !matches!(s.phase, Phase::Done) {
        return Next::Finish(TaskStatus::OutOfSteps);
    }
    let mut g = GoalInfo::read(goal.text);
    let app = s.app.clone().unwrap_or_default();
    // Once the app is open its name is spent: "new finder window" shouldn't match "open downloads in finder".
    if let Some(a) = s.apps.as_ref().and_then(|a| a.apps.iter().find(|x| x.id == app)) {
        if matches!(s.phase, Phase::ChooseMove | Phase::Descend { .. }) {
            // Only words that are the app's name as written ("finder"), not a singular that happens to match it
            // ("note" in "a new note" for Notes): the first token of each name word is its surface form.
            for (w, _, _) in words(goal.text) {
                let surface = crate::ax::tok_hash(&w);
                if a.names.contains(&surface) {
                    let forms: Vec<String> = crate::ax::match_forms(&w)
                        .iter()
                        .map(|f| crate::ax::tok_hash(f))
                        .collect();
                    g.targets.retain(|t| !forms.contains(t));
                }
            }
        }
    }
    match &s.phase {
        Phase::Catalog => Next::Catalog,
        Phase::ChooseApp => Next::Ask(q_app(&g, s, labels)),
        Phase::Act { action } => {
            if s.executed() >= MAX_ACTIONS {
                Next::Finish(TaskStatus::OutOfSteps)
            } else {
                Next::Act(action.clone())
            }
        }
        Phase::AfterAction => Next::Tree { app, scope: None },
        Phase::Verify => match focused_text_field(s) {
            Some(n) if !s.typed.contains(&n.id) && !g.cands.is_empty() => {
                Next::Ask(q_text(&g, s, n, app_is_browser(s, &app)))
            }
            _ => Next::Ask(q_verify(&g, s)),
        },
        Phase::ChooseMove => {
            if s.tree.is_none() {
                Next::Tree { app, scope: None }
            } else if !s.shortcuts.contains_key(&app) {
                Next::Shortcuts { app }
            } else {
                Next::Ask(q_move(&g, s, labels))
            }
        }
        Phase::Descend { path } => {
            let Some(t) = s.tree.as_ref() else {
                return Next::Tree { app, scope: None };
            };
            match level(&t.root, path) {
                Level::Children(c) if !c.is_empty() => Next::Ask(q_element(&g, s, &c, labels)),
                Level::Children(_) | Level::Gone => Next::Backtrack(BacktrackReason::NoneOfThese),
                Level::NeedObserve(h) => Next::Tree { app, scope: Some(h) },
            }
        }
        Phase::Done => Next::Finish(TaskStatus::Done),
        Phase::Failed { reason } => Next::Backtrack(*reason),
        Phase::Finished => Next::Wait,
    }
}

fn focused_text_field(s: &CtrlState) -> Option<&AxNode> {
    let t = s.tree.as_ref()?;
    let n = find(&t.root, t.focused.as_deref()?)?;
    n.text_input.then_some(n)
}

// ---------------------------------------------------------------------------------------------------------------
// Questions

fn finish(node: Node, mut cs: Vec<Choice>) -> Question {
    // Highest prior first; ties keep source order. Then the cap, then "none of these".
    let mut idx: Vec<usize> = (0..cs.len()).collect();
    idx.sort_by(|a, b| cs[*b].prior.cmp(&cs[*a].prior).then(a.cmp(b)));
    let mut seen = BTreeSet::new();
    let mut out: Vec<Choice> = Vec::new();
    for i in idx {
        if out.len() >= MAX_ENTRIES {
            break;
        }
        if seen.insert(cs[i].opt.id.clone()) {
            out.push(std::mem::replace(
                &mut cs[i],
                Choice {
                    opt: Opt::none(),
                    prior: 0,
                    label: String::new(),
                },
            ));
        }
    }
    out.push(Choice {
        opt: Opt::none(),
        prior: NONE_PRIOR,
        label: "none of these".into(),
    });
    Question { node, choices: out }
}

fn q_app(g: &GoalInfo, s: &CtrlState, labels: &dyn Labels) -> Question {
    let Some(apps) = s.apps.as_ref() else {
        return finish(Node::ChooseApp, vec![]);
    };
    let address =
        g.has_address() || (g.wants_search && g.targets.iter().any(|t| *t == crate::ax::tok_hash("web")));
    let cs = apps
        .apps
        .iter()
        .map(|a| {
            let toks: BTreeSet<String> = a.tokens.iter().cloned().collect();
            let (t, _) = g.overlap(&toks);
            let mut prior = t * 4000;
            if t > 0 && a.tokens.iter().all(|x| g.targets.contains(x) || x.is_empty()) {
                prior += 300;
            }
            if a.browser && address {
                prior += if apps.default_browser.as_deref() == Some(a.id.as_str()) {
                    4500
                } else {
                    2000
                };
            }
            if a.running {
                prior += 100;
            }
            Choice {
                opt: Opt::new(format!("app.{}", a.id), OptionKind::App),
                prior,
                label: labels.get(&a.id).unwrap_or_else(|| a.id.clone()).to_lowercase(),
            }
        })
        .collect();
    finish(Node::ChooseApp, cs)
}

fn opt_ax(n: &AxNode, act: &str, kind: OptionKind) -> Opt {
    let mut o = Opt::new(format!("ax.{}.{act}", n.id), kind);
    o.role = Some(role_slug(&n.role, n.subrole.as_deref()));
    o.gated = !n.risk.is_empty() && act != "focus";
    o
}

fn node_label(n: &AxNode, labels: &dyn Labels) -> String {
    let role = role_slug(&n.role, n.subrole.as_deref()).replace('_', " ");
    match labels.get(&n.id) {
        Some(t) => format!("{} ({role})", t.to_lowercase()),
        None => role,
    }
}

/// The entries of one level as choices: branches and clusters to descend into, element actions to take.
fn level_choices(g: &GoalInfo, kids: &[&AxNode], labels: &dyn Labels) -> Vec<Choice> {
    let mut cs = Vec::new();
    let es = entries(kids);
    let roomy = es.len() < MAX_ENTRIES;
    for e in es {
        match e {
            Entry::Group {
                role,
                page,
                pages,
                total,
                members,
            } => {
                let mut toks = BTreeSet::new();
                for m in &members {
                    subtree_tokens(m, &mut toks);
                }
                let (t, v) = g.overlap(&toks);
                let mut o = Opt::new(format!("group.{role}~{page}"), OptionKind::Group);
                o.role = Some(role.clone());
                let r = role.replace('_', " ");
                cs.push(Choice {
                    opt: o,
                    prior: t.min(3) * 2500 + v.min(2) * 300,
                    label: if pages > 1 {
                        format!(
                            "{r}s {}–{} of {total}",
                            page as usize * MAX_ENTRIES + 1,
                            page as usize * MAX_ENTRIES + members.len()
                        )
                    } else {
                        format!("{total} {r}s")
                    },
                });
            }
            Entry::Node(n) => {
                let own: BTreeSet<String> = n.tokens.iter().chain(&n.value_tokens).cloned().collect();
                let (t, v) = g.overlap(&own);
                let label = node_label(n, labels);
                let toggle = g.wants_toggle && TOGGLE_ROLES.contains(&n.role.as_str());
                let base = t * 4000 + v * 800 + g.risk_penalty(&n.risk) + if toggle { 1500 } else { 0 };
                if n.text_input {
                    let search = n.subrole.as_deref() == Some("AXSearchField")
                        || n.role == "AXSearchField"
                        || n.tokens.contains(&crate::ax::tok_hash("search"));
                    let mut prior = base;
                    if !g.cands.is_empty() {
                        prior += 1500;
                    }
                    if search && g.wants_search {
                        prior += 2000;
                    }
                    cs.push(Choice {
                        opt: opt_ax(n, "focus", OptionKind::ElementAction),
                        prior,
                        label: format!("type in {label}"),
                    });
                }
                let has = |a: &str| n.actions.iter().any(|x| x == a);
                if has("AXPress") {
                    cs.push(Choice {
                        opt: opt_ax(n, "press", OptionKind::ElementAction),
                        prior: base + 100,
                        label: format!("press {label}"),
                    });
                } else if has("AXShowMenu") {
                    cs.push(Choice {
                        opt: opt_ax(n, "show_menu", OptionKind::ElementAction),
                        prior: base,
                        label: format!("open the menu of {label}"),
                    });
                }
                if has("AXIncrement") {
                    cs.push(Choice {
                        opt: opt_ax(n, "increment", OptionKind::ElementAction),
                        prior: base,
                        label: format!("increase {label}"),
                    });
                }
                if has("AXDecrement") {
                    cs.push(Choice {
                        opt: opt_ax(n, "decrement", OptionKind::ElementAction),
                        prior: base,
                        label: format!("decrease {label}"),
                    });
                }
                if !n.children.is_empty() || n.more > 0 {
                    let mut toks = BTreeSet::new();
                    subtree_tokens(n, &mut toks);
                    let (t, v) = g.overlap(&toks);
                    let mut o = Opt::new(format!("branch.{}", n.id), OptionKind::Branch);
                    o.role = Some(role_slug(&n.role, n.subrole.as_deref()));
                    let count = n.children.len().max(n.more as usize);
                    cs.push(Choice {
                        opt: o,
                        prior: t.min(3) * 2500 + v.min(2) * 300,
                        label: format!("{label}: {count} items"),
                    });
                    if roomy && matches!(n.role.as_str(), "AXScrollArea" | "AXWebArea") {
                        cs.push(Choice {
                            opt: opt_ax(n, "scroll_down", OptionKind::ElementAction),
                            prior: -500,
                            label: format!("scroll down {label}"),
                        });
                    }
                }
            }
        }
    }
    cs
}

fn q_move(g: &GoalInfo, s: &CtrlState, labels: &dyn Labels) -> Question {
    let app = s.app.clone().unwrap_or_default();
    let browser = app_is_browser(s, &app);
    let menu = s.shortcuts.get(&app).cloned().unwrap_or_default();
    let menu_combos: Vec<String> = menu.iter().map(|m| m.combo.clone()).collect();
    let mut cs: Vec<Choice> = Vec::new();
    // (a) The app's own menu bar: only items that match the goal, the best few.
    let mut m: Vec<Choice> = menu
        .iter()
        .filter(|m| combo_allowed(&m.combo, &menu_combos, browser))
        .filter_map(|m| {
            let toks: BTreeSet<String> = m.tokens.iter().cloned().collect();
            let (t, v) = g.overlap(&toks);
            (t + v > 0).then(|| {
                let mut o = Opt::new(format!("menu.{}", m.id), OptionKind::MenuItem);
                o.gated = !m.risk.is_empty();
                Choice {
                    opt: o,
                    // The menu bar is the first source (spec §3): a matching item beats the same button in the window.
                    prior: t * 2500 + v * 1200 + if t > 0 { 2500 } else { 200 } + g.risk_penalty(&m.risk),
                    label: format!(
                        "{} ({})",
                        labels
                            .get(&m.id)
                            .unwrap_or_else(|| "menu item".into())
                            .to_lowercase(),
                        m.combo
                    ),
                }
            })
        })
        .collect();
    m.sort_by(|a, b| b.prior.cmp(&a.prior));
    m.truncate(MENU_KEEP);
    cs.extend(m);
    // (b) The built-in table.
    for (combo, label, only_browser) in BUILTIN_SHORTCUTS {
        if *only_browser && !browser {
            continue;
        }
        let prior = match *combo {
            "cmd+l" if g.has_address() || g.wants_search => 5000,
            "cmd+f" if g.wants_search && !browser => 1800,
            "cmd+t" if g.wants_tab => 2500,
            "cmd+n" if g.wants_new => 1500,
            "cmd+comma" if g.wants_settings => 2500,
            _ => 0,
        };
        let mut o = Opt::new(format!("key.{combo}"), OptionKind::Shortcut);
        o.gated = COMBO_RULES.iter().any(|(k, _)| k == combo);
        cs.push(Choice {
            opt: o,
            prior,
            label: format!("{label} ({combo})"),
        });
    }
    // (c) Open the address directly (a browser and an address in the goal).
    if browser {
        if let Some(c) = g
            .cands
            .iter()
            .find(|c| matches!(c.kind, TextKind::Domain | TextKind::Site))
        {
            let mut o = Opt::new("url.0", OptionKind::OpenUrl);
            o.label_ref = Some(c.label_ref.clone());
            cs.push(Choice {
                opt: o,
                prior: 1500,
                label: "open the address".into(),
            });
        }
    }
    // (d) The front window's top level.
    if let Some(t) = s.tree.as_ref() {
        let kids: Vec<&AxNode> = t.root.children.iter().collect();
        cs.extend(level_choices(g, &kids, labels));
    }
    finish(Node::ChooseMove, cs)
}

fn q_element(g: &GoalInfo, _s: &CtrlState, kids: &[&AxNode], labels: &dyn Labels) -> Question {
    finish(Node::ChooseElement, level_choices(g, kids, labels))
}

fn q_text(g: &GoalInfo, s: &CtrlState, field: &AxNode, browser: bool) -> Question {
    let address = browser
        && (field
            .tokens
            .iter()
            .any(|t| [crate::ax::tok_hash("address"), crate::ax::tok_hash("url")].contains(t))
            || matches!(
                s.last_action().map(|a| &a.action),
                Some(Action::KeyCombo { combo, .. }) if combo == "cmd+l"
            ));
    let search = field.subrole.as_deref() == Some("AXSearchField")
        || field.role == "AXSearchField"
        || field.tokens.contains(&crate::ax::tok_hash("search"));
    let cs = g
        .cands
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let (prior, submit) = match c.kind {
                TextKind::Domain | TextKind::Site if address => (3500, true),
                TextKind::Domain | TextKind::Site => (500, false),
                TextKind::Search if search => (3500, true),
                TextKind::Search if address => (2000, true),
                TextKind::Search => (1500, true),
                TextKind::Literal if address || search => (1500, true),
                TextKind::Literal => (3000, false),
            };
            let mut o = Opt::new(
                format!("text.{i}{}", if submit { ".go" } else { "" }),
                OptionKind::TextCandidate,
            );
            o.label_ref = Some(c.label_ref.clone());
            Choice {
                opt: o,
                prior,
                label: String::new(),
            }
        })
        .collect();
    finish(Node::ChooseText, cs)
}

fn salient(t: &AxNode, focused: Option<&str>, out: &mut BTreeSet<String>) {
    out.extend(t.tokens.iter().cloned());
    fn walk(n: &AxNode, focused: Option<&str>, out: &mut BTreeSet<String>) {
        out.extend(n.value_tokens.iter().cloned());
        if Some(n.id.as_str()) == focused || n.role == "AXHeading" || n.role == "AXWebArea" {
            out.extend(n.tokens.iter().cloned());
        }
        for c in &n.children {
            walk(c, focused, out);
        }
    }
    walk(t, focused, out);
}

fn q_verify(g: &GoalInfo, s: &CtrlState) -> Question {
    let (mut done, mut cont, mut failed) = (0i64, 4000i64, 0i64);
    let last = s.last_action().filter(|a| a.status == ActionStatus::Done);
    let changed = match (&s.tree_before, s.tree.as_ref()) {
        (Some(b), Some(t)) => *b != t.hash,
        _ => true,
    };
    let app_toks: BTreeSet<String> = s
        .apps
        .as_ref()
        .and_then(|a| a.apps.iter().find(|x| Some(&x.id) == s.app.as_ref()))
        .map(|a| a.tokens.iter().cloned().collect())
        .unwrap_or_default();
    let beyond_app: Vec<&String> = g.targets.iter().filter(|t| !app_toks.contains(*t)).collect();
    let mut seen = BTreeSet::new();
    if let Some(t) = s.tree.as_ref() {
        salient(&t.root, t.focused.as_deref(), &mut seen);
    }
    let reached = !beyond_app.is_empty() && beyond_app.iter().all(|t| seen.contains(*t));
    // Or the element just pressed is what the goal names ("press send": the send button, now gone).
    let acted: BTreeSet<&String> = s.acted_tokens.iter().collect();
    let covered =
        !beyond_app.is_empty() && beyond_app.iter().all(|t| seen.contains(*t) || acted.contains(*t));
    match last.map(|a| &a.action) {
        Some(Action::OpenApp { .. }) if beyond_app.is_empty() => {
            done = 4000;
            cont = 0;
        }
        Some(Action::Focus { .. }) => {}
        Some(_) if !changed => {
            failed = 4000;
            cont = 0;
            done = -1000;
        }
        Some(a) => {
            let committed = match a {
                Action::SetValue { submit, .. } => *submit || !g.cands.is_empty(),
                Action::KeyCombo { combo, .. } => !matches!(combo.as_str(), "cmd+l" | "cmd+f"),
                Action::Press { .. }
                | Action::OpenUrl { .. }
                | Action::Increment { .. }
                | Action::Decrement { .. } => true,
                _ => false,
            };
            let pressed = matches!(a, Action::Press { .. } | Action::KeyCombo { .. });
            let toggled =
                !g.wants_toggle || s.acted_role.as_deref().is_some_and(|r| TOGGLE_ROLES.contains(&r));
            if committed && toggled && (reached || (pressed && covered)) {
                done = 4000;
                cont = 500;
                failed = -1000;
            }
        }
        None => {}
    }
    let mk = |id: &str, prior: i64, label: &str| Choice {
        opt: Opt::new(id, OptionKind::Verdict),
        prior,
        label: label.into(),
    };
    let cs = vec![
        mk("verify.done", done, "it's done"),
        mk("verify.continue", cont, "keep going"),
        mk("verify.failed", failed, "that didn't work"),
    ];
    // Verdicts keep their order (no re-ranking needed: three options and "none of these").
    let mut q = finish(Node::Verify, cs);
    q.choices.sort_by_key(|c| match c.opt.id.as_str() {
        "verify.done" => 0,
        "verify.continue" => 1,
        "verify.failed" => 2,
        _ => 3,
    });
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_reading_finds_addresses_searches_and_titles() {
        let g = "open youtube";
        let gi = GoalInfo::read(g);
        assert_eq!(gi.cands.len(), 1);
        assert_eq!(resolve(&gi.cands[0].label_ref, g).as_deref(), Some("youtube.com"));
        let g = "open a new safari tab to apple.com";
        let gi = GoalInfo::read(g);
        assert!(gi.wants_tab && gi.wants_new);
        let d = gi.cands.iter().find(|c| c.kind == TextKind::Domain).unwrap();
        assert_eq!(resolve(&d.label_ref, g).as_deref(), Some("apple.com"));
        let g = "search notes for groceries";
        let gi = GoalInfo::read(g);
        let s = gi.cands.iter().find(|c| c.kind == TextKind::Search).unwrap();
        assert_eq!(resolve(&s.label_ref, g).as_deref(), Some("groceries"));
        let g = "create a new note titled \"test\"";
        let gi = GoalInfo::read(g);
        assert_eq!(resolve(&gi.cands[0].label_ref, g).as_deref(), Some("test"));
        let g = "find \"invoice\" in mail";
        assert_eq!(
            resolve(&GoalInfo::read(g).cands[0].label_ref, g).as_deref(),
            Some("invoice")
        );
        assert_eq!(resolve("goal#99..120", "short"), None);
    }
}

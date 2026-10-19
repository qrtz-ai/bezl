//! The accessibility tree (ADR 0214; decision-layer spec §4, §6). The helper reads the front window of an app as a
//! nested tree of elements, each with an opaque handle (`e:<pid>:<n>`) that only the helper's own element cache can
//! resolve. That is the wire ([`AxNodeWire`]): it carries titles and, for plain text inputs, values, and it never
//! reaches the log. [`prune`] turns it into the log form ([`AxNode`]):
//!
//! - **never** a secure text field (`AXSecureTextField`), and never a password or payment field (a text input whose
//!   label names a password, passcode, card number, security code, expiry, IBAN, account or routing number…): they
//!   are dropped with their subtree and only counted;
//! - hidden, zero-size and disabled leaves are dropped; unlabelled `AXGroup` wrappers with no action are collapsed
//!   into their parent;
//! - titles and values become hashed word tokens ([`tok_hash`], singulars included), so the chain can match them against the goal and a
//!   replay sees the same tree without one word of what was on screen (ADR 0083);
//! - the gate's label rules are evaluated here, on the folded label, and kept as rule ids ([`AxNode::risk`]).
//!
//! [`entries`] presents one level as ≤ 19 entries: a level with more children is grouped by role into clusters
//! ("list: 140 rows", paged by 19), and choosing a cluster descends into it, so the narrowing is recursive.
use crate::gate::{fold_label, label_risk, sensitive_field};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Entries one level offers before "none of these" (spec §1: ≤ 20 options).
pub const MAX_ENTRIES: usize = 19;
/// Most nodes one observation keeps (the rest are counted in `more`).
pub const MAX_TREE_NODES: usize = 2000;

/// The AX actions the chain can map to its fixed action set ([`crate::act::AxAction`]).
pub const KNOWN_AX_ACTIONS: &[&str] = &["AXPress", "AXIncrement", "AXDecrement", "AXShowMenu"];
/// Roles that take typed text (focus, then `choose_text`, then set value).
pub const TEXT_INPUT_ROLES: &[&str] = &["AXTextField", "AXTextArea", "AXSearchField", "AXComboBox"];
/// Never read, never offered, never acted on.
pub const SECURE_ROLES: &[&str] = &["AXSecureTextField"];
/// Wrappers collapsed into their parent when they have no label and no action.
const WRAPPER_ROLES: &[&str] = &["AXGroup", "AXUnknown", "AXLayoutArea"];

/// One element as the helper sends it (the wire, never the log).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AxNodeWire {
    pub id: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrole: Option<String>,
    /// Title, description or label, whichever the element has.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub placeholder: String,
    /// Only for plain text inputs and a web area's URL; never for a secure field (the helper doesn't read it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub hidden: bool,
    /// Width and height in points (`None`: unknown, kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 2]>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub children: Vec<AxNodeWire>,
    /// Children the helper didn't expand (deeper than it read): descending asks for them.
    #[serde(default)]
    pub more: u32,
    /// The helper's fingerprint of the live element (role, subrole, title, actions); re-checked before acting.
    #[serde(default)]
    pub sig: String,
}

fn yes() -> bool {
    true
}

/// One element as the log holds it. No titles, no values: hashed tokens only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxNode {
    pub id: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrole: Option<String>,
    pub tokens: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub value_tokens: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub text_input: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    /// The gate's label rules this element's label matches (rule ids, never the label).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<AxNode>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sig: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// What pruning dropped, counted (the log says how much was hidden from the chain and why, never what).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dropped {
    pub secure: u32,
    pub sensitive: u32,
    pub hidden: u32,
    pub disabled: u32,
    pub collapsed: u32,
    pub bad_id: u32,
    pub over_cap: u32,
}

/// An observation as logged: the pruned tree of `app`'s front window (`scope: None`) or one element's children.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeObs {
    pub app: String,
    pub scope: Option<String>,
    pub root: AxNode,
    pub focused: Option<String>,
    pub hash: String,
    pub dropped: Dropped,
}

/// Lowercase alphanumeric words of `text` with their byte spans (anything else separates words).
pub fn words(text: &str) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push((text[s..i].to_lowercase(), s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((text[s..].to_lowercase(), s, text.len()));
    }
    out
}

/// Hash of a lowercase word: how labels are matched without being logged.
pub fn tok_hash(w: &str) -> String {
    let h = Sha256::digest(format!("qrtz-ctrl-tok:{w}").as_bytes());
    hex::encode(&h[..6])
}

/// Hashed tokens of a label: the folded words (homoglyphs and invisible characters folded, ADR 0214 gate), deduped
/// in order.
pub fn label_tokens(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    words(&fold_label(text))
        .into_iter()
        .flat_map(|(w, _, _)| match_forms(&w))
        .map(|w| tok_hash(&w))
        .filter(|h| seen.insert(h.clone()))
        .collect()
}

/// A word and its singular ("notes" → "notes", "note"), so labels and goals match across plurals.
pub fn match_forms(w: &str) -> Vec<String> {
    let mut v = vec![w.to_string()];
    if w.chars().count() > 3 && w.ends_with('s') && !w.ends_with("ss") {
        v.push(w[..w.len() - 1].to_string());
    }
    v
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b':' || b == b'-' || b == b'_')
}

fn is_secure(n: &AxNodeWire) -> bool {
    SECURE_ROLES.contains(&n.role.as_str()) || n.subrole.as_deref().is_some_and(|s| SECURE_ROLES.contains(&s))
}

pub fn is_text_input(role: &str) -> bool {
    TEXT_INPUT_ROLES.contains(&role)
}

struct Pruner {
    dropped: Dropped,
    kept: usize,
}

impl Pruner {
    /// Prune one wire node; `None` = dropped. Collapsed wrappers return their children through `out`.
    fn node(&mut self, n: &AxNodeWire, out: &mut Vec<AxNode>) {
        if is_secure(n) {
            self.dropped.secure += 1;
            return;
        }
        let text_input = is_text_input(&n.role);
        if text_input && sensitive_field(&format!("{} {}", n.title, n.placeholder)) {
            self.dropped.sensitive += 1;
            return;
        }
        if !valid_id(&n.id) {
            self.dropped.bad_id += 1;
            return;
        }
        if n.hidden || n.size.is_some_and(|[w, h]| w <= 0.0 || h <= 0.0) {
            self.dropped.hidden += 1;
            return;
        }
        if self.kept >= MAX_TREE_NODES {
            self.dropped.over_cap += 1;
            return;
        }
        let actions: Vec<String> = n
            .actions
            .iter()
            .filter(|a| KNOWN_AX_ACTIONS.contains(&a.as_str()))
            .cloned()
            .collect();
        let mut children = Vec::new();
        for c in &n.children {
            self.node(c, &mut children);
        }
        let label = format!("{} {}", n.title, n.placeholder);
        let unlabelled = label.trim().is_empty();
        if WRAPPER_ROLES.contains(&n.role.as_str()) && unlabelled && actions.is_empty() && n.more == 0 {
            self.dropped.collapsed += 1;
            out.extend(children);
            return;
        }
        if !n.enabled && children.is_empty() && n.more == 0 {
            self.dropped.disabled += 1;
            return;
        }
        self.kept += 1;
        let value_tokens = match (&n.value, text_input || n.role == "AXWebArea") {
            (Some(v), true) => label_tokens(v),
            _ => Vec::new(),
        };
        out.push(AxNode {
            id: n.id.clone(),
            role: n.role.clone(),
            subrole: n.subrole.clone(),
            tokens: label_tokens(&label),
            value_tokens,
            actions: if n.enabled { actions } else { Vec::new() },
            text_input: text_input && n.enabled,
            focused: n.focused,
            risk: label_risk(&n.title),
            children,
            more: n.more,
            sig: n.sig.clone(),
        });
    }
}

/// The readable labels of a wire tree, handle → title (memory only, for the UI and the Laya prompt).
pub fn wire_labels(n: &AxNodeWire, out: &mut BTreeMap<String, String>) {
    if is_secure(n) || (is_text_input(&n.role) && sensitive_field(&format!("{} {}", n.title, n.placeholder)))
    {
        return;
    }
    let t = if n.title.trim().is_empty() {
        n.placeholder.trim()
    } else {
        n.title.trim()
    };
    if !t.is_empty() {
        out.insert(n.id.clone(), t.chars().take(80).collect());
    }
    for c in &n.children {
        wire_labels(c, out);
    }
}

/// Prune a wire tree into the log form. The root (the window, or the scope element) is always kept, even when it
/// would collapse; a secure or sensitive root becomes an empty node.
pub fn prune(app: &str, scope: Option<&str>, root: &AxNodeWire, focused: Option<&str>) -> TreeObs {
    let mut p = Pruner {
        dropped: Dropped::default(),
        kept: 0,
    };
    let mut kids = Vec::new();
    let unsafe_root = is_secure(root) || !valid_id(&root.id);
    if unsafe_root {
        p.dropped.secure += u32::from(is_secure(root));
    } else {
        for c in &root.children {
            p.node(c, &mut kids);
        }
    }
    let r = AxNode {
        id: if unsafe_root {
            "root".into()
        } else {
            root.id.clone()
        },
        role: root.role.clone(),
        subrole: root.subrole.clone(),
        tokens: if unsafe_root {
            vec![]
        } else {
            label_tokens(&root.title)
        },
        value_tokens: Vec::new(),
        actions: Vec::new(),
        text_input: false,
        focused: false,
        risk: Vec::new(),
        children: kids,
        more: if unsafe_root { 0 } else { root.more },
        sig: root.sig.clone(),
    };
    // A focused handle the pruned tree doesn't hold (a secure field) isn't reported.
    let focused = focused.filter(|f| find(&r, f).is_some()).map(str::to_string);
    let hash = tree_hash(&r);
    TreeObs {
        app: app.into(),
        scope: scope.map(str::to_string),
        root: r,
        focused,
        hash,
        dropped: p.dropped,
    }
}

/// Content hash of a pruned tree (what a confirm approval binds to).
pub fn tree_hash(root: &AxNode) -> String {
    hex::encode(&Sha256::digest(serde_json::to_vec(root).expect("serializes"))[..8])
}

pub fn find<'a>(n: &'a AxNode, id: &str) -> Option<&'a AxNode> {
    if n.id == id {
        return Some(n);
    }
    n.children.iter().find_map(|c| find(c, id))
}

fn find_mut<'a>(n: &'a mut AxNode, id: &str) -> Option<&'a mut AxNode> {
    if n.id == id {
        return Some(n);
    }
    n.children.iter_mut().find_map(|c| find_mut(c, id))
}

/// Put a scoped observation's children under its scope element; returns false when the element is gone.
pub fn merge(root: &mut AxNode, scope: &str, sub: &AxNode) -> bool {
    match find_mut(root, scope) {
        Some(n) => {
            n.children = sub.children.clone();
            n.more = sub.more;
            true
        }
        None => false,
    }
}

/// Every token in a subtree (labels and values), for the priors.
pub fn subtree_tokens(n: &AxNode, out: &mut BTreeSet<String>) {
    out.extend(n.tokens.iter().cloned());
    out.extend(n.value_tokens.iter().cloned());
    for c in &n.children {
        subtree_tokens(c, out);
    }
}

pub fn count_nodes(n: &AxNode) -> usize {
    1 + n.children.iter().map(count_nodes).sum::<usize>()
}

/// What changed between two trees: element ids added, removed, or changed in themselves.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxDiff {
    pub added: u32,
    pub removed: u32,
    pub changed: u32,
    /// The focused element moved.
    pub focus_moved: bool,
}

impl AxDiff {
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0 && self.changed == 0 && !self.focus_moved
    }
}

fn flat<'a>(n: &'a AxNode, out: &mut BTreeMap<&'a str, &'a AxNode>) {
    out.insert(n.id.as_str(), n);
    for c in &n.children {
        flat(c, out);
    }
}

fn shallow_eq(a: &AxNode, b: &AxNode) -> bool {
    a.role == b.role
        && a.subrole == b.subrole
        && a.tokens == b.tokens
        && a.value_tokens == b.value_tokens
        && a.actions == b.actions
        && a.children.len() == b.children.len()
}

pub fn diff(before: &TreeObs, after: &TreeObs) -> AxDiff {
    let mut a = BTreeMap::new();
    let mut b = BTreeMap::new();
    flat(&before.root, &mut a);
    flat(&after.root, &mut b);
    AxDiff {
        added: b.keys().filter(|k| !a.contains_key(*k)).count() as u32,
        removed: a.keys().filter(|k| !b.contains_key(*k)).count() as u32,
        changed: a
            .iter()
            .filter(|(k, n)| b.get(*k).is_some_and(|m| !shallow_eq(n, m)))
            .count() as u32,
        focus_moved: before.focused != after.focused,
    }
}

/// The lowercase role word an option's label carries (`close_button`, `toolbar`, `text_field`).
pub fn role_slug(role: &str, subrole: Option<&str>) -> String {
    let pick = match subrole {
        Some(
            s @ ("AXCloseButton" | "AXMinimizeButton" | "AXZoomButton" | "AXTabButton" | "AXSearchField"),
        ) => s,
        _ => role,
    };
    let base = pick.strip_prefix("AX").unwrap_or(pick);
    let mut out = String::new();
    for (i, c) in base.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// One entry of a level: an element, or a cluster of same-role elements (one page of at most 19).
#[derive(Debug, Clone, PartialEq)]
pub enum Entry<'a> {
    Node(&'a AxNode),
    Group {
        role: String,
        page: u32,
        pages: u32,
        total: u32,
        members: Vec<&'a AxNode>,
    },
}

/// One level as at most [`MAX_ENTRIES`] entries is the target; more children are grouped by role (in the order the
/// roles first appear), each cluster paged by 19. Singletons stay elements. The chain's priors then truncate to the
/// top 19.
pub fn entries<'a>(children: &[&'a AxNode]) -> Vec<Entry<'a>> {
    if children.len() <= MAX_ENTRIES {
        return children.iter().map(|n| Entry::Node(n)).collect();
    }
    let mut order: Vec<String> = Vec::new();
    let mut by_role: BTreeMap<String, Vec<&'a AxNode>> = BTreeMap::new();
    for n in children {
        let r = role_slug(&n.role, n.subrole.as_deref());
        if !by_role.contains_key(&r) {
            order.push(r.clone());
        }
        by_role.entry(r).or_default().push(n);
    }
    let mut out = Vec::new();
    for r in order {
        let members = &by_role[&r];
        if members.len() == 1 {
            out.push(Entry::Node(members[0]));
            continue;
        }
        let pages = members.len().div_ceil(MAX_ENTRIES) as u32;
        for (p, chunk) in members.chunks(MAX_ENTRIES).enumerate() {
            out.push(Entry::Group {
                role: r.clone(),
                page: p as u32,
                pages,
                total: members.len() as u32,
                members: chunk.to_vec(),
            });
        }
    }
    out
}

/// One step down the narrowed path: an element (its children) or a cluster of the level above.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum ScopeStep {
    Node { id: String },
    Group { role: String, page: u32 },
}

/// Where a path leads: the level's children, or an element whose children must be read first.
#[derive(Debug, Clone, PartialEq)]
pub enum Level<'a> {
    Children(Vec<&'a AxNode>),
    NeedObserve(String),
    Gone,
}

/// Walk `path` from the root.
pub fn level<'a>(root: &'a AxNode, path: &[ScopeStep]) -> Level<'a> {
    let mut cur: Vec<&AxNode> = root.children.iter().collect();
    for step in path {
        match step {
            ScopeStep::Node { id } => {
                let Some(n) = cur
                    .iter()
                    .find(|n| n.id == *id)
                    .copied()
                    .or_else(|| cur.iter().find_map(|n| find(n, id)))
                else {
                    return Level::Gone;
                };
                if n.children.is_empty() && n.more > 0 {
                    return Level::NeedObserve(n.id.clone());
                }
                cur = n.children.iter().collect();
            }
            ScopeStep::Group { role, page } => {
                let es = entries(&cur);
                let Some(members) = es.into_iter().find_map(|e| match e {
                    Entry::Group {
                        role: r,
                        page: p,
                        members,
                        ..
                    } if r == *role && p == *page => Some(members),
                    _ => None,
                }) else {
                    return Level::Gone;
                };
                cur = members;
            }
        }
    }
    Level::Children(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: &str, role: &str, title: &str) -> AxNodeWire {
        AxNodeWire {
            id: id.into(),
            role: role.into(),
            title: title.into(),
            enabled: true,
            actions: vec!["AXPress".into()],
            ..Default::default()
        }
    }

    #[test]
    fn secure_and_payment_fields_never_enter_the_tree_or_the_labels() {
        let mut root = w("e:1:0", "AXWindow", "checkout");
        let mut pw = w("e:1:1", "AXTextField", "");
        pw.subrole = Some("AXSecureTextField".into());
        pw.value = Some("hunter2".into());
        let mut card = w("e:1:2", "AXTextField", "card number");
        card.value = Some("4111 1111 1111 1111".into());
        let mut cvc = w("e:1:3", "AXTextField", "");
        cvc.placeholder = "CVC".into();
        root.children = vec![pw, card, cvc, w("e:1:4", "AXButton", "pay")];
        let t = prune("com.example", None, &root, Some("e:1:1"));
        assert_eq!(t.root.children.len(), 1);
        assert_eq!((t.dropped.secure, t.dropped.sensitive), (1, 2));
        assert_eq!(t.focused, None, "a focused secure field isn't reported");
        let mut labels = BTreeMap::new();
        wire_labels(&root, &mut labels);
        assert!(!labels.contains_key("e:1:2") && !labels.contains_key("e:1:3"));
        let json = serde_json::to_string(&t).unwrap();
        for bad in ["hunter2", "4111", "CVC", "Card"] {
            assert!(!json.contains(bad), "{bad} in {json}");
        }
        assert_eq!(
            json.matches("pay").count(),
            1,
            "only as the host rule id label:pay"
        );
    }

    #[test]
    fn wrappers_collapse_and_big_levels_group_by_role() {
        let mut root = w("e:1:0", "AXWindow", "mail");
        let mut wrap = AxNodeWire {
            id: "e:1:9".into(),
            role: "AXGroup".into(),
            enabled: true,
            ..Default::default()
        };
        wrap.children = (0..45)
            .map(|i| w(&format!("e:1:{}", 100 + i), "AXRow", "msg"))
            .collect();
        root.children = vec![wrap, w("e:1:5", "AXButton", "reply")];
        let t = prune("com.apple.mail", None, &root, None);
        assert_eq!(t.dropped.collapsed, 1);
        assert_eq!(t.root.children.len(), 46);
        let kids: Vec<&AxNode> = t.root.children.iter().collect();
        let es = entries(&kids);
        assert_eq!(es.len(), 4, "three pages of rows and the button");
        assert!(
            matches!(&es[0], Entry::Group { role, pages: 3, total: 45, members, .. } if role == "row" && members.len() == 19)
        );
        let path = [ScopeStep::Group {
            role: "row".into(),
            page: 2,
        }];
        assert!(matches!(level(&t.root, &path), Level::Children(c) if c.len() == 7));
    }
}

/// One menu-bar item with a key equivalent, as `ctrl.menu.shortcuts` sends it (the wire).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuShortcutWire {
    /// The menu item's handle (pressing it is `press` on this handle).
    pub id: String,
    /// Menu titles from the bar down to the item ("file", "new tab").
    pub path: Vec<String>,
    pub combo: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// The same, as logged: hashed tokens of the item's title (and its menu's), the combo, the gate's label rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutObs {
    pub id: String,
    pub combo: String,
    pub tokens: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk: Vec<String>,
}

/// Most menu shortcuts kept per app (the move question keeps the best few by prior).
pub const MAX_SHORTCUTS: usize = 200;

/// The log form of an app's menu shortcuts: enabled items with a valid handle and a combo the policy can send.
pub fn shortcuts_obs(items: &[MenuShortcutWire]) -> Vec<ShortcutObs> {
    items
        .iter()
        .filter(|m| m.enabled && valid_id(&m.id) && !m.combo.trim().is_empty())
        .take(MAX_SHORTCUTS)
        .map(|m| ShortcutObs {
            id: m.id.clone(),
            combo: crate::act::normalize_combo(&m.combo),
            tokens: label_tokens(m.path.last().map(String::as_str).unwrap_or("")),
            risk: label_risk(m.path.last().map(String::as_str).unwrap_or("")),
        })
        .collect()
}

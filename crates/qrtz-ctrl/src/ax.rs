//! AX observations (slice 5, ADR 0108): what the helper saw of a background app's front window, in the log's
//! text-free form. The helper sends titles over the wire (never logged); [`AxSnapshot::from_wire`] turns them into
//! hashed tokens (the same [`tok_hash`] the app catalog uses), so a snapshot can be matched against the goal and
//! replayed without holding a word of what was on screen (ADR 0083).
//!
//! Narrowing (Jared's design, spec §1): the first snapshot of a run is the front window's top-level branches (`scope
//! = None`); choosing a branch with children asks the helper for that branch's children (`scope = Some(branch)`),
//! so no question ever looks at more than [`MAX_NODES`] elements.
use crate::layers::{tok_hash, tokens};
use crate::policy::{MENU_OPENING_ROLES, SECURE_ROLES};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Most elements one snapshot keeps (the move question has routes and shortcuts too, and stays ≤ 20 options).
pub const MAX_NODES: usize = 12;

/// One element as the helper sends it (the wire, not the log).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AxNodeWire {
    /// The helper's opaque handle for the live element (`e:<hash>`), valid for the helper's element cache.
    pub id: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrole: Option<String>,
    /// Title / description / help, whichever the element has. Never a value of a text field.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub children: u32,
}

/// One element as the log holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxNode {
    pub id: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrole: Option<String>,
    /// Hashed lowercase title tokens.
    pub tokens: Vec<String>,
    pub actions: Vec<String>,
    pub children: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxSnapshot {
    pub app: String,
    /// `None`: the front window's top level; `Some(id)`: that element's children.
    pub scope: Option<String>,
    pub nodes: Vec<AxNode>,
    /// Elements the helper saw beyond [`MAX_NODES`] (dropped, counted).
    pub truncated: u32,
}

/// Elements ctrl never offers: secure fields and anything that opens a menu (ADR 0100 refuses them anyway).
fn offerable(n: &AxNodeWire) -> bool {
    // Handles are `e:<hash>`; a `.` would break the `ax.<handle>.<role>` option id.
    !n.id.is_empty()
        && !n.id.contains('.')
        && !SECURE_ROLES.contains(&n.role.as_str())
        && !MENU_OPENING_ROLES.contains(&n.role.as_str())
}

impl AxSnapshot {
    pub fn from_wire(app: &str, scope: Option<&str>, nodes: &[AxNodeWire]) -> AxSnapshot {
        let keep: Vec<&AxNodeWire> = nodes.iter().filter(|n| offerable(n)).collect();
        let truncated = keep.len().saturating_sub(MAX_NODES) as u32;
        AxSnapshot {
            app: app.into(),
            scope: scope.map(str::to_string),
            nodes: keep
                .into_iter()
                .take(MAX_NODES)
                .map(|n| AxNode {
                    id: n.id.clone(),
                    role: n.role.clone(),
                    subrole: n.subrole.clone(),
                    tokens: tokens(&n.title).into_iter().map(|t| tok_hash(&t.w)).collect(),
                    actions: n.actions.clone(),
                    children: n.children,
                })
                .collect(),
            truncated,
        }
    }

    pub fn node(&self, id: &str) -> Option<&AxNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// What changed between two snapshots of the same scope: element ids added, removed, or with different
/// role / tokens / actions / child count.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

impl AxDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

pub fn diff(before: &AxSnapshot, after: &AxSnapshot) -> AxDiff {
    let a: BTreeMap<&str, &AxNode> = before.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let b: BTreeMap<&str, &AxNode> = after.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    AxDiff {
        added: b
            .keys()
            .filter(|k| !a.contains_key(*k))
            .map(|k| k.to_string())
            .collect(),
        removed: a
            .keys()
            .filter(|k| !b.contains_key(*k))
            .map(|k| k.to_string())
            .collect(),
        changed: a
            .iter()
            .filter(|(k, n)| b.get(*k).is_some_and(|m| m != *n))
            .map(|(k, _)| k.to_string())
            .collect(),
    }
}

/// The lowercase role word a branch option's id and label carry (`close_button`, `toolbar`, `tab_group`).
pub fn role_slug(n: &AxNode) -> String {
    let pick = match n.subrole.as_deref() {
        Some(s @ ("AXCloseButton" | "AXMinimizeButton" | "AXZoomButton" | "AXTabButton")) => s,
        _ => n.role.as_str(),
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

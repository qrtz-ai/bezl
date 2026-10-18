//! The confirm gate (slice 6, ADR 0111): Bound → Confirm → Act (decision-layer spec §9).
//!
//! Before an irreversible plan acts, the run stops with `confirm.requested` and a concrete preview of what it would
//! touch: the action, the app, and every window or tab it affects (titles are hashed into item keys before they
//! reach the log; the readable titles live only in memory for the view). The user approves, denies, or approves
//! and remembers it for this task: a standing approval scoped to (intent, move, app) and bounded by the preview's
//! item and unsaved counts. Every answer, expiry, standing-approval use and the re-check before acting are events,
//! so a replay is exact and a fork from before the answer asks again.
//!
//! Pure: the preview's inputs come from the helper ([`crate::exec`]); everything here is comparison and hashing.
use crate::decide::Node;
use crate::helper::{PreviewItemWire, PreviewWhat};
use crate::log::CtrlState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// A pending gate expires after this long without an answer (a scheduled run while the user is away waits this
/// long, then ends `expired`; nothing acts). A newer run of the same task supersedes it sooner.
pub const CONFIRM_TTL_MS: u64 = 12 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Window,
    Tab,
    /// No enumerable items (a setting, a send): the action itself is the one item.
    Action,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewItem {
    pub kind: ItemKind,
    /// `sha256(title)[..12]#occurrence`, or `n<i>` when the helper only gave a count.
    pub key: String,
    /// `None`: unknown, counted as unsaved (the gate errs toward asking).
    pub unsaved: Option<bool>,
}

/// What an irreversible step would do, as logged. No titles, no task text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    /// The chosen intent option (`intent.quit_app`) and move (`shortcut.cmd+q`).
    pub intent: String,
    #[serde(rename = "move")]
    pub mv: String,
    pub app: Option<String>,
    pub items: Vec<PreviewItem>,
    /// `true`: the helper enumerated the items; `false`: only a count was available.
    pub complete: bool,
}

/// What a standing approval covers: this intent and move in this app, up to this many items and unsaved items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub intent: String,
    #[serde(rename = "move")]
    pub mv: String,
    pub app: Option<String>,
    pub items: u32,
    pub unsaved: u32,
}

impl Scope {
    /// `other` fits inside this scope (same action and app, no more items, no more unsaved ones).
    pub fn covers(&self, other: &Scope) -> bool {
        self.intent == other.intent
            && self.mv == other.mv
            && self.app == other.app
            && other.items <= self.items
            && other.unsaved <= self.unsaved
    }
}

impl Preview {
    pub fn scope(&self) -> Scope {
        Scope {
            intent: self.intent.clone(),
            mv: self.mv.clone(),
            app: self.app.clone(),
            items: self.items.len() as u32,
            unsaved: self.items.iter().filter(|i| i.unsaved != Some(false)).count() as u32,
        }
    }

    /// The re-check right before acting: the world now (`self`) is still inside what was approved. Fewer items is
    /// fine (something the user closed meanwhile); a new item, more unsaved work or another app re-asks.
    pub fn within(&self, approved: &Preview) -> bool {
        if !approved.scope().covers(&self.scope()) {
            return false;
        }
        if self.complete && approved.complete {
            return self.items.iter().all(|i| {
                approved
                    .items
                    .iter()
                    .any(|a| a.key == i.key && (a.unsaved == i.unsaved || i.unsaved == Some(false)))
            });
        }
        self.complete == approved.complete
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpiredWhy {
    Timeout,
    Superseded,
}

/// How a gate was resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum ConfirmAnswer {
    Approve,
    Deny,
    /// Approved, and a standing approval for this task was granted (store id).
    ApproveRemember {
        approval_id: String,
        scope: Scope,
    },
    /// A standing approval of this task covered the preview: no question asked. The scope is copied in so the
    /// run's log replays without the task store.
    Standing {
        approval_id: String,
        scope: Scope,
    },
    Expired {
        why: ExpiredWhy,
    },
}

impl ConfirmAnswer {
    pub fn approves(&self) -> bool {
        matches!(
            self,
            ConfirmAnswer::Approve | ConfirmAnswer::ApproveRemember { .. } | ConfirmAnswer::Standing { .. }
        )
    }
}

/// A standing approval of one task (kept in the task store, `approval.granted` / `approval.revoked`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub scope: Scope,
    pub granted_at: u64,
}

/// The gate as the state holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateState {
    pub id: String,
    pub preview: Preview,
    pub rules: Vec<String>,
    pub answer: Option<ConfirmAnswer>,
    /// The re-check before acting: `Some(true)` = still inside the approval.
    pub reverified: Option<bool>,
    /// The approved action was requested: the approval is spent (approve acts once).
    pub used: bool,
}

/// Selector (spec §13.3): the run waits for the user's answer.
pub fn needs_confirm(s: &CtrlState) -> bool {
    s.confirm.as_ref().is_some_and(|g| g.answer.is_none())
}

/// What the preview enumerates for an intent.
pub fn preview_what(intent: &str) -> Option<(PreviewWhat, ItemKind)> {
    match intent {
        "intent.quit_app" => Some((PreviewWhat::Windows, ItemKind::Window)),
        "intent.close_tabs" => Some((PreviewWhat::Tabs, ItemKind::Tab)),
        "intent.close_window" => Some((PreviewWhat::Windows, ItemKind::Window)),
        _ => None,
    }
}

fn title_key(title: &str) -> String {
    hex::encode(&Sha256::digest(title.as_bytes())[..6])
}

/// Hash the helper's items into preview items. Returns the items and `key → title` for the in-memory label cache.
pub fn hash_items(kind: ItemKind, wire: &[PreviewItemWire]) -> (Vec<PreviewItem>, Vec<(String, String)>) {
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let mut items = Vec::with_capacity(wire.len());
    let mut labels = Vec::with_capacity(wire.len());
    for w in wire {
        let base = title_key(&w.title);
        let n = seen.entry(base.clone()).or_insert(0);
        let key = format!("{base}#{n}");
        *n += 1;
        labels.push((key.clone(), w.title.clone()));
        items.push(PreviewItem {
            kind,
            key,
            unsaved: w.unsaved,
        });
    }
    (items, labels)
}

/// Items from a bare count (the helper couldn't enumerate): unknown unsaved state.
pub fn counted_items(kind: ItemKind, n: i64) -> Vec<PreviewItem> {
    (0..n.max(0))
        .map(|i| PreviewItem {
            kind,
            key: format!("n{i}"),
            unsaved: None,
        })
        .collect()
}

/// The rules that put the plan behind the gate (`choose_intent:intent.quit_app`, …), from the irreversible marks.
pub fn rules(s: &CtrlState) -> Vec<String> {
    let Some(plan) = s.plan.as_ref() else {
        return Vec::new();
    };
    plan.steps
        .iter()
        .filter(|st| {
            s.decisions
                .iter()
                .filter(|d| d.node == st.node)
                .any(|d| d.options.iter().any(|o| o.id == st.option && o.irreversible))
        })
        .map(|st| format!("{}:{}", st.node.as_str(), st.option))
        .collect()
}

/// The chosen intent and move of the plan.
pub fn intent_and_move(s: &CtrlState) -> (String, String) {
    let c = s.chosen();
    (
        c.get(&Node::ChooseIntent).cloned().unwrap_or_default(),
        c.get(&Node::ChooseMove).cloned().unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(items: &[(&str, Option<bool>)], complete: bool) -> Preview {
        Preview {
            intent: "intent.quit_app".into(),
            mv: "shortcut.cmd+q".into(),
            app: Some("com.apple.TextEdit".into()),
            items: items
                .iter()
                .map(|(k, u)| PreviewItem {
                    kind: ItemKind::Window,
                    key: (*k).into(),
                    unsaved: *u,
                })
                .collect(),
            complete,
        }
    }

    #[test]
    fn scope_covers_only_the_same_action_with_no_more_items() {
        let a = p(&[("a", Some(false)), ("b", Some(false))], true).scope();
        assert!(a.covers(&p(&[("x", Some(false))], true).scope()));
        assert!(!a.covers(
            &p(
                &[("a", Some(false)), ("b", Some(false)), ("c", Some(false))],
                true
            )
            .scope()
        ));
        assert!(
            !a.covers(&p(&[("a", Some(true))], true).scope()),
            "unsaved work grows the scope"
        );
        assert!(
            !a.covers(&p(&[("a", None)], true).scope()),
            "unknown counts as unsaved"
        );
        let mut other_app = p(&[("a", Some(false))], true);
        other_app.app = Some("com.apple.Safari".into());
        assert!(!a.covers(&other_app.scope()));
    }

    #[test]
    fn within_allows_fewer_items_but_not_new_ones() {
        let approved = p(&[("a", Some(false)), ("b", Some(false))], true);
        assert!(p(&[("a", Some(false))], true).within(&approved));
        assert!(
            !p(&[("a", Some(false)), ("c", Some(false))], true).within(&approved),
            "a new window re-asks"
        );
        assert!(
            !p(&[("a", Some(true))], true).within(&approved),
            "now unsaved re-asks"
        );
        assert!(
            !p(&[("n0", None)], false).within(&approved),
            "losing the enumeration re-asks"
        );
    }

    #[test]
    fn same_titles_get_distinct_keys() {
        let w = |t: &str| PreviewItemWire {
            title: t.into(),
            unsaved: Some(false),
        };
        let (items, labels) = hash_items(ItemKind::Window, &[w("untitled"), w("untitled"), w("notes")]);
        assert_eq!(items.len(), 3);
        assert_ne!(items[0].key, items[1].key);
        assert!(
            items.iter().all(|i| !i.key.contains("untitled")),
            "no titles in keys"
        );
        assert_eq!(labels[2].1, "notes");
    }
}

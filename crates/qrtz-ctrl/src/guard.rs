//! The UI guard (ADR 0100): what the user owns, snapshotted before and after every ctrl action.
//!
//! A snapshot holds only ids and coordinates: the frontmost app's pid, the cursor position, an opaque id of the
//! focused UI element (a hash, never its label or value: ADR 0083) and the key window's number. `None` means the
//! probe could not read that field (no Accessibility trust, no frontmost app); an unreadable field is reported as
//! inconclusive, never as a violation and never as "unchanged".
//!
//! `hid_idle_ms` is the time since the last *physical* input event (`CGEventSourceSecondsSinceLastEventType` on the
//! HID system state). Events ctrl posts to a pid don't pass through the HID state, so if it dropped below the
//! action's duration the user touched the mouse or keyboard during the action, and any change is attributed to them.
use crate::policy::CURSOR_TOLERANCE_PT;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UiSnapshot {
    pub frontmost_pid: Option<i32>,
    pub cursor: Option<Point>,
    /// Opaque id of the system-wide focused element (`e:<hash>`), never its text.
    pub focused_element: Option<String>,
    /// Window number of the frontmost app's key (focused) window.
    pub key_window: Option<u32>,
    pub hid_idle_ms: Option<u64>,
}

/// One thing the user owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    FrontmostApp,
    Cursor,
    FocusedElement,
    KeyWindow,
}

impl Field {
    pub const ALL: [Field; 4] = [
        Field::FrontmostApp,
        Field::Cursor,
        Field::FocusedElement,
        Field::KeyWindow,
    ];
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GuardOutcome {
    /// Fields ctrl changed. Any entry is a violation of ADR 0100.
    pub violations: Vec<Field>,
    /// Fields that couldn't be judged: unreadable on either side, or changed while the user was active.
    pub inconclusive: Vec<Field>,
    /// The user physically touched the mouse or keyboard during the action.
    pub user_input: bool,
}

impl GuardOutcome {
    pub fn clean(&self) -> bool {
        self.violations.is_empty()
    }
}

enum Cmp {
    Same,
    Changed,
    Unknown,
}

fn cmp<T: PartialEq>(a: &Option<T>, b: &Option<T>) -> Cmp {
    match (a, b) {
        (Some(a), Some(b)) if a == b => Cmp::Same,
        (Some(_), Some(_)) => Cmp::Changed,
        _ => Cmp::Unknown,
    }
}

fn cmp_cursor(a: &Option<Point>, b: &Option<Point>) -> Cmp {
    match (a, b) {
        (Some(a), Some(b)) => {
            if (a.x - b.x).abs() <= CURSOR_TOLERANCE_PT && (a.y - b.y).abs() <= CURSOR_TOLERANCE_PT {
                Cmp::Same
            } else {
                Cmp::Changed
            }
        }
        _ => Cmp::Unknown,
    }
}

/// Compare two snapshots around an action that took `elapsed_ms`.
pub fn compare(before: &UiSnapshot, after: &UiSnapshot, elapsed_ms: u64) -> GuardOutcome {
    let user_input = after.hid_idle_ms.is_some_and(|idle| idle < elapsed_ms);
    let mut out = GuardOutcome {
        user_input,
        ..Default::default()
    };
    for f in Field::ALL {
        let c = match f {
            Field::FrontmostApp => cmp(&before.frontmost_pid, &after.frontmost_pid),
            Field::Cursor => cmp_cursor(&before.cursor, &after.cursor),
            Field::FocusedElement => cmp(&before.focused_element, &after.focused_element),
            Field::KeyWindow => cmp(&before.key_window, &after.key_window),
        };
        match c {
            Cmp::Same => {}
            Cmp::Unknown => out.inconclusive.push(f),
            Cmp::Changed if user_input => out.inconclusive.push(f),
            Cmp::Changed => out.violations.push(f),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Case {
        name: String,
        elapsed_ms: u64,
        before: UiSnapshot,
        after: UiSnapshot,
        violations: Vec<Field>,
        inconclusive: Vec<Field>,
        user_input: bool,
    }

    #[derive(Deserialize)]
    struct Cases {
        cases: Vec<Case>,
    }

    #[test]
    fn shared_guard_cases() {
        let p = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../native/qrtz-helper/Fixtures/ctrl/guard-cases.json"
        );
        let cases: Cases = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert!(cases.cases.len() >= 8);
        for c in cases.cases {
            let got = compare(&c.before, &c.after, c.elapsed_ms);
            assert_eq!(got.violations, c.violations, "{}: violations", c.name);
            assert_eq!(got.inconclusive, c.inconclusive, "{}: inconclusive", c.name);
            assert_eq!(got.user_input, c.user_input, "{}: user_input", c.name);
        }
    }

    #[test]
    fn a_swift_snapshot_with_unreadable_fields_decodes() {
        // The Swift probe omits nil fields ("{}" when it can read nothing).
        assert_eq!(
            serde_json::from_str::<UiSnapshot>("{}").unwrap(),
            UiSnapshot::default()
        );
        let s: UiSnapshot = serde_json::from_str(r#"{"frontmost_pid":5,"cursor":{"x":1,"y":2}}"#).unwrap();
        assert_eq!(
            (s.frontmost_pid, s.cursor, s.key_window),
            (Some(5), Some(Point { x: 1.0, y: 2.0 }), None)
        );
    }

    #[test]
    fn an_unreadable_snapshot_is_never_clean_by_accident() {
        let out = compare(&UiSnapshot::default(), &UiSnapshot::default(), 10);
        assert!(out.violations.is_empty());
        assert_eq!(out.inconclusive, Field::ALL.to_vec());
    }
}

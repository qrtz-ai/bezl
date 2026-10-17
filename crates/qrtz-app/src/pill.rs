//! When the talk pill is on screen (Jared: it must go the instant the key is released). Pure, so the shell
//! (`apps/desktop/src-tauri/src/backend.rs::on_talk`) and the E2E (`tests/e2e_talk.rs`) use the same rule.
//!
//! - `listening` (the key is held): shown.
//! - Key released (`finalizing` / `cleaning` / `inserting`): hidden at once. The final text, cleanup and insert
//!   finish in the background, and the text is inserted when it's ready.
//! - The dictation failed (`error`, or a `last` entry that says the text couldn't be inserted): a brief flash of
//!   the pill with the error ([`ERROR_FLASH`]), or an OS notification when the pill is turned off. Each failure
//!   flashes once ([`PillGate`]).
//! - Review §12.22 (a): the `last` failure counts in any phase, not only `idle`. Since ADR 0074 an older
//!   dictation's insert can finish (and fail) while a newer one is finalizing, inserting or still listening. While
//!   the key is held the pill shows the live text, so the flash waits for the release; with the pill off the
//!   notification is posted at once.
use serde_json::Value;
use std::time::Duration;

pub const ERROR_FLASH: Duration = Duration::from_millis(2500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PillAction {
    Show,
    Hide,
    /// Show the error for `ERROR_FLASH`, then hide. `key` identifies the failure so a re-emitted view doesn't
    /// flash it again. `notify`: the pill is off (`talk.showPill`), so tell the OS instead.
    Flash {
        key: String,
        error: String,
        notify: bool,
    },
}

/// `last` says its text couldn't be inserted: (`"{last.id}:{error}"`, error).
fn last_failure(talk: &Value) -> Option<(String, String)> {
    (talk["last"]["inserted"] == false)
        .then(|| talk["last"]["error"].as_str())
        .flatten()
        .map(|e| {
            (
                format!("{}:{e}", talk["last"]["id"].as_str().unwrap_or("")),
                e.to_owned(),
            )
        })
}

pub fn pill_action(config: &Value, talk: &Value) -> PillAction {
    let show_pill = config["talk"]["showPill"].as_bool().unwrap_or(true);
    let failure = match talk["phase"].as_str().unwrap_or("idle") {
        "listening" if show_pill => return PillAction::Show,
        "error" => talk["error"]
            .as_str()
            .map(|e| {
                let id = talk["id"].as_str().or(talk["last"]["id"].as_str()).unwrap_or("");
                (format!("{id}:{e}"), e.to_owned())
            })
            .or_else(|| last_failure(talk)),
        _ => last_failure(talk),
    };
    match failure {
        Some((key, error)) => PillAction::Flash {
            key,
            error,
            notify: !show_pill,
        },
        None => PillAction::Hide,
    }
}

/// [`pill_action`] with each failure flashed (or notified) once: a view that repeats the failure already shown is
/// `Hide`, so a dictation that starts after it is still hidden on release. The shell keeps one per app.
#[derive(Debug, Default)]
pub struct PillGate {
    flashed: Option<String>,
}

impl PillGate {
    pub fn next(&mut self, config: &Value, talk: &Value) -> PillAction {
        match pill_action(config, talk) {
            PillAction::Flash { key, .. } if self.flashed.as_deref() == Some(key.as_str()) => {
                PillAction::Hide
            }
            PillAction::Flash { key, error, notify } => {
                self.flashed = Some(key.clone());
                PillAction::Flash { key, error, notify }
            }
            a => a,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shown_only_while_the_key_is_held_and_flashed_on_failure() {
        let on = json!({"talk": {"showPill": true}});
        let off = json!({"talk": {"showPill": false}});
        assert_eq!(pill_action(&on, &json!({"phase": "listening"})), PillAction::Show);
        assert_eq!(
            pill_action(&off, &json!({"phase": "listening"})),
            PillAction::Hide
        );
        for p in ["finalizing", "inserting", "idle"] {
            assert_eq!(
                pill_action(&on, &json!({"phase": p, "id": "d1"})),
                PillAction::Hide,
                "{p}"
            );
        }
        let err = json!({"phase": "error", "id": "d1", "error": "offline", "errorKind": "offline"});
        assert_eq!(
            pill_action(&on, &err),
            PillAction::Flash {
                key: "d1:offline".into(),
                error: "offline".into(),
                notify: false
            }
        );
        assert!(matches!(
            pill_action(&off, &err),
            PillAction::Flash { notify: true, .. }
        ));
        let not_inserted = json!({"phase": "idle", "last": {"id": "d2", "inserted": false,
                                  "error": "no focused field; copied to clipboard"}});
        assert_eq!(
            pill_action(&on, &not_inserted),
            PillAction::Flash {
                key: "d2:no focused field; copied to clipboard".into(),
                error: "no focused field; copied to clipboard".into(),
                notify: false
            }
        );
        let inserted = json!({"phase": "idle", "last": {"id": "d3", "inserted": true}});
        assert_eq!(pill_action(&on, &inserted), PillAction::Hide);
    }

    /// §12.22 (a): an older dictation's failed insert lands while a newer one is finalizing or inserting (or while
    /// the key is held): it flashes on release, or notifies at once with the pill off, and only once.
    #[test]
    fn a_new_failed_last_entry_flashes_in_any_phase_once() {
        let on = json!({"talk": {"showPill": true}});
        let off = json!({"talk": {"showPill": false}});
        let last = json!({"id": "d1", "inserted": false, "error": "couldn't insert; copied to clipboard"});
        let key = "d1:couldn't insert; copied to clipboard";
        let mut g = PillGate::default();
        let held = json!({"phase": "listening", "id": "d2", "last": last});
        assert_eq!(
            g.next(&on, &held),
            PillAction::Show,
            "the live pill isn't replaced"
        );
        for p in ["finalizing", "inserting"] {
            let v = json!({"phase": p, "id": "d2", "last": last});
            assert!(
                matches!(pill_action(&on, &v), PillAction::Flash { ref key, notify: false, .. } if key == "d1:couldn't insert; copied to clipboard"),
                "{p}"
            );
        }
        assert!(matches!(
            g.next(&on, &json!({"phase": "finalizing", "id": "d2", "last": last})),
            PillAction::Flash { .. }
        ));
        assert_eq!(
            g.next(&on, &json!({"phase": "inserting", "id": "d2", "last": last})),
            PillAction::Hide,
            "once"
        );
        assert_eq!(
            g.next(&on, &json!({"phase": "idle", "last": last})),
            PillAction::Hide
        );
        assert_eq!(
            g.next(&on, &json!({"phase": "listening", "id": "d3", "last": last})),
            PillAction::Show
        );

        let mut g = PillGate::default();
        assert_eq!(
            g.next(&off, &held),
            PillAction::Flash {
                key: key.into(),
                error: "couldn't insert; copied to clipboard".into(),
                notify: true
            },
            "pill off: notified while the key is held"
        );
        assert_eq!(
            g.next(&off, &json!({"phase": "finalizing", "id": "d2", "last": last})),
            PillAction::Hide
        );
        assert_eq!(
            g.next(&off, &json!({"phase": "listening", "id": "d2"})),
            PillAction::Hide
        );
    }
}

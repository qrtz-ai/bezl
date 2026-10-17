//! When the talk pill is on screen (Jared: it must go the instant the key is released). Pure, so the shell
//! (`apps/desktop/src-tauri/src/backend.rs::on_talk`) and the E2E (`tests/e2e_talk.rs`) use the same rule.
//!
//! - `listening` (the key is held): shown.
//! - Key released (`finalizing` / `cleaning` / `inserting`): hidden at once. The final text, cleanup and insert
//!   finish in the background, and the text is inserted when it's ready.
//! - The dictation failed (`error`, or `idle` whose `last` entry says the text couldn't be inserted): a brief
//!   flash of the pill with the error ([`ERROR_FLASH`]), or an OS notification when the pill is turned off.
//!   Each failure flashes once ([`PillAction::Flash::key`]).
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

pub fn pill_action(config: &Value, talk: &Value) -> PillAction {
    let show_pill = config["talk"]["showPill"].as_bool().unwrap_or(true);
    let failure = match talk["phase"].as_str().unwrap_or("idle") {
        "listening" => {
            return if show_pill {
                PillAction::Show
            } else {
                PillAction::Hide
            }
        }
        "error" => talk["error"].as_str().map(|e| {
            let id = talk["id"].as_str().or(talk["last"]["id"].as_str()).unwrap_or("");
            (format!("{id}:{e}"), e.to_owned())
        }),
        "idle" => (talk["last"]["inserted"] == false)
            .then(|| talk["last"]["error"].as_str())
            .flatten()
            .map(|e| {
                (
                    format!("{}:{e}", talk["last"]["id"].as_str().unwrap_or("")),
                    e.to_owned(),
                )
            }),
        _ => None,
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
}

//! ADR 0092: the helper draws the pill from its own talk state; the shell uses [`PillGate`] for what only it knows
//! (a flash forwarded as `pill.flash`, or a notification with the pill off).
//!
//! When the talk pill is on screen. Pure, so the shell (`apps/desktop/src-tauri/src/backend.rs::on_talk`) and
//! the E2E (`tests/e2e_talk.rs`) use the same rule.
//!
//! Jared (Oct 17, reversing ADR 0071's "hide on release"): the pill stays up until the dictation is done.
//! - `listening`, `finalizing`, `inserting` (key held, then the final text and the insert): shown.
//! - Done (`idle`, the text is in): hidden.
//! - The dictation failed (`error`, or a `last` entry that says the text couldn't be inserted): a brief flash of
//!   the pill with the error ([`ERROR_FLASH`]), then hidden; an OS notification instead when the pill is turned
//!   off. Each failure flashes once ([`PillGate`]).
//! - Review §12.22 (a): an older dictation's insert can finish (and fail) while a newer one is busy (ADR 0074).
//!   While the pill shows the newer dictation, that failure waits and flashes once the pill would hide; with the
//!   pill off the notification is posted at once.
use serde_json::Value;
use std::time::Duration;

pub const ERROR_FLASH: Duration = Duration::from_millis(2500);

/// ADR 0092: what the helper's native pill shows for a flash: the clipboard fallback is a notice ("copied to
/// clipboard", info), anything else an error, lowercase.
pub fn flash_kind(error: &str) -> (&'static str, String) {
    let e = error.trim().to_lowercase();
    if e.ends_with("copied to clipboard") {
        ("info", "copied to clipboard".to_owned())
    } else {
        ("error", e)
    }
}

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

/// A dictation is in flight (the pill shows it while `talk.showPill` is on).
fn busy(phase: &str) -> bool {
    matches!(phase, "listening" | "finalizing" | "inserting")
}

pub fn pill_action(config: &Value, talk: &Value) -> PillAction {
    let show_pill = config["talk"]["showPill"].as_bool().unwrap_or(true);
    let phase = talk["phase"].as_str().unwrap_or("idle");
    if show_pill && busy(phase) {
        return PillAction::Show;
    }
    let failure = match phase {
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

/// [`pill_action`] with each failure flashed (or notified) once. A failure that lands while the pill is showing a
/// newer dictation is kept and flashed when that one is done. The shell keeps one per app.
///
/// N145: a refusal (`phase: error`) is one attempt. Its flash is deduplicated only while the view stays on that
/// error (a re-emitted view); once the view leaves the error phase, the same refusal on the next press flashes
/// again. A `last` failure (an insert) stays deduplicated for good: `last` is kept across views.
#[derive(Debug, Default)]
pub struct PillGate {
    flashed: Option<String>,
    error_flashed: Option<String>,
    pending: Option<(String, String)>,
}

impl PillGate {
    pub fn next(&mut self, config: &Value, talk: &Value) -> PillAction {
        let attempt_error = talk["phase"] == "error" && talk["error"].is_string();
        if !attempt_error {
            self.error_flashed = None;
        }
        match pill_action(config, talk) {
            PillAction::Show => {
                if let Some((key, error)) = last_failure(talk) {
                    if self.flashed.as_deref() != Some(key.as_str()) {
                        self.pending = Some((key, error));
                    }
                }
                PillAction::Show
            }
            PillAction::Flash { key, .. }
                if self.flashed.as_deref() == Some(key.as_str())
                    || self.error_flashed.as_deref() == Some(key.as_str()) =>
            {
                self.take_pending()
            }
            PillAction::Flash { key, error, notify } => {
                if self.pending.as_ref().is_some_and(|(k, _)| *k == key) {
                    self.pending = None;
                }
                if attempt_error {
                    self.error_flashed = Some(key.clone());
                } else {
                    self.flashed = Some(key.clone());
                }
                PillAction::Flash { key, error, notify }
            }
            PillAction::Hide => self.take_pending(),
        }
    }

    /// A failure kept while the pill was busy, flashed now (once); else hide.
    fn take_pending(&mut self) -> PillAction {
        match self.pending.take() {
            Some((key, error)) if self.flashed.as_deref() != Some(key.as_str()) => {
                self.flashed = Some(key.clone());
                PillAction::Flash {
                    key,
                    error,
                    notify: false,
                }
            }
            _ => PillAction::Hide,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_clipboard_fallback_is_info() {
        assert_eq!(
            flash_kind("No focused field; copied to clipboard"),
            ("info", "copied to clipboard".to_owned())
        );
        assert_eq!(flash_kind("Offline"), ("error", "offline".to_owned()));
        // There is no cleanup step any more (ADR 0071): a stale "cleaning" phase is not a busy pill.
        assert_eq!(
            pill_action(&json!({}), &json!({"phase": "cleaning", "id": "d1"})),
            PillAction::Hide
        );
    }

    #[test]
    fn shown_until_the_dictation_is_done_and_flashed_on_failure() {
        let on = json!({"talk": {"showPill": true}});
        let off = json!({"talk": {"showPill": false}});
        for p in ["listening", "finalizing", "inserting"] {
            assert_eq!(
                pill_action(&on, &json!({"phase": p, "id": "d1"})),
                PillAction::Show,
                "{p}"
            );
            assert_eq!(
                pill_action(&off, &json!({"phase": p, "id": "d1"})),
                PillAction::Hide,
                "{p}"
            );
        }
        assert_eq!(pill_action(&on, &json!({"phase": "idle"})), PillAction::Hide);
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

    /// §12.22 (a): an older dictation's failed insert lands while a newer one is busy: with the pill on it waits
    /// for the newer one to be done and flashes once; with the pill off it notifies at once, once.
    #[test]
    fn a_failure_during_a_newer_dictation_flashes_once_when_it_is_done() {
        let on = json!({"talk": {"showPill": true}});
        let off = json!({"talk": {"showPill": false}});
        let last = json!({"id": "d1", "inserted": false, "error": "couldn't insert; copied to clipboard"});
        let key = "d1:couldn't insert; copied to clipboard";
        let ok = json!({"id": "d2", "inserted": true});
        let mut g = PillGate::default();
        for p in ["listening", "finalizing", "inserting"] {
            assert_eq!(
                g.next(&on, &json!({"phase": p, "id": "d2", "last": last})),
                PillAction::Show,
                "{p}"
            );
        }
        // d2 is in; its own success replaced `last`, but d1's failure still flashes, once.
        assert!(matches!(
            g.next(&on, &json!({"phase": "idle", "last": ok})),
            PillAction::Flash { key: ref k, notify: false, .. } if k == key
        ));
        assert_eq!(
            g.next(&on, &json!({"phase": "idle", "last": ok})),
            PillAction::Hide
        );
        assert_eq!(
            g.next(&on, &json!({"phase": "idle", "last": last})),
            PillAction::Hide,
            "once"
        );
        assert_eq!(
            g.next(&on, &json!({"phase": "listening", "id": "d3", "last": last})),
            PillAction::Show
        );
        assert_eq!(
            g.next(&on, &json!({"phase": "idle", "last": last})),
            PillAction::Hide
        );

        let mut g = PillGate::default();
        let held = json!({"phase": "listening", "id": "d2", "last": last});
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
            g.next(&off, &json!({"phase": "idle", "last": ok})),
            PillAction::Hide
        );
    }

    #[test]
    fn the_same_refusal_on_the_next_press_flashes_again() {
        // N145: refusal, idle, the same refusal: each press is told (was Flash, Hide, Hide).
        let cfg = json!({});
        let refused =
            json!({"phase": "error", "error": "speech isn't ready", "last": {"id": "d1", "inserted": true}});
        let idle = json!({"phase": "idle", "last": {"id": "d1", "inserted": true}});
        let mut g = PillGate::default();
        assert!(matches!(g.next(&cfg, &refused), PillAction::Flash { .. }));
        assert_eq!(
            g.next(&cfg, &refused),
            PillAction::Hide,
            "a re-emitted view doesn't flash twice"
        );
        assert_eq!(g.next(&cfg, &idle), PillAction::Hide);
        assert!(
            matches!(g.next(&cfg, &refused), PillAction::Flash { .. }),
            "the next press's refusal flashes"
        );
        // An insert failure in `last` stays flashed once across views.
        let failed = json!({"phase": "idle", "last": {"id": "d2", "inserted": false, "error": "no focus"}});
        assert!(matches!(g.next(&cfg, &failed), PillAction::Flash { .. }));
        assert_eq!(g.next(&cfg, &idle), PillAction::Hide);
        assert_eq!(g.next(&cfg, &failed), PillAction::Hide);
    }
}

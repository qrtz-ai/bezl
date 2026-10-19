//! The stop key and "user input wins" (ADR 0214, amending ADR 0100 §4–§5).
//!
//! ctrl do may take the foreground: activate apps, press ⌘L, type. While a run is acting, the helper watches
//! physical input (`ctrl.observe {arm: true}`), telling Jared's events apart from the ones it posted itself:
//!
//! - **Esc is the stop key.** One physical press of Esc stops the run at once: nothing more is acted, the run ends
//!   `stopped`. ctrl never sends Esc itself (it's reserved in [`crate::act::RESERVED_COMBOS`]).
//! - **User input wins.** Any other physical key press, click or scroll while the run acts also stops it, ended
//!   `stopped` with `user_input`: Jared took over, and ctrl doesn't fight him for the keyboard. Moving the mouse
//!   alone doesn't count.
//!
//! The watcher is armed only while the run acts and disarmed while it asks or waits for a confirm (Jared clicking
//! "approve" is not taking over). The host checks the flags before and after every action; the helper also refuses
//! `ctrl.ax.act` once its watcher has tripped, so a stop wins even against a request already on the wire.
use serde::{Deserialize, Serialize};

/// What `ctrl.observe` reports. Ids only: the frontmost app's bundle id and the focused element's handle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontmost: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused: Option<String>,
    /// Esc pressed physically since the watcher was armed.
    #[serde(default)]
    pub stopped: bool,
    /// Any other physical key press, click or scroll since the watcher was armed.
    #[serde(default)]
    pub user_input: bool,
    /// The watcher is running (`false`: it couldn't start, e.g. no event-tap permission; the host then won't act).
    #[serde(default)]
    pub armed: bool,
    /// Review (d): why an unarmed watcher couldn't start: `accessibility`, `input_monitoring`,
    /// `accessibility_and_input_monitoring` or `secure_input`
    /// (none: unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Esc.
    StopKey,
    /// Jared typed, clicked or scrolled.
    UserInput,
    /// The stop button in ctrl's view.
    StopButton,
    /// The watcher couldn't be armed, so ctrl can't promise the stop key works: it doesn't act.
    NoWatcher,
    /// Review (d): no watcher because qrtz has no Accessibility permission.
    NoAccessibility,
    /// Review (d): no watcher because qrtz has no Input Monitoring permission.
    NoInputMonitoring,
    /// Review (d): neither Accessibility nor Input Monitoring is granted, so both are named.
    NoAccessibilityAndInputMonitoring,
    /// Review (d): no watcher because secure input is on (a password field or prompt has the keyboard).
    SecureInput,
}

/// The guard's verdict on one observation.
pub fn verdict(o: &Observed) -> Option<StopReason> {
    if o.stopped {
        Some(StopReason::StopKey)
    } else if o.user_input {
        Some(StopReason::UserInput)
    } else if !o.armed {
        Some(match o.missing.as_deref() {
            Some("accessibility") => StopReason::NoAccessibility,
            Some("input_monitoring") => StopReason::NoInputMonitoring,
            Some("accessibility_and_input_monitoring") => StopReason::NoAccessibilityAndInputMonitoring,
            Some("secure_input") => StopReason::SecureInput,
            _ => StopReason::NoWatcher,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_beats_user_input_and_an_unarmed_watcher_never_acts() {
        let o = |stopped, user_input, armed| Observed {
            stopped,
            user_input,
            armed,
            ..Default::default()
        };
        assert_eq!(verdict(&o(false, false, true)), None);
        assert_eq!(verdict(&o(true, true, true)), Some(StopReason::StopKey));
        assert_eq!(verdict(&o(false, true, true)), Some(StopReason::UserInput));
        assert_eq!(verdict(&o(false, false, false)), Some(StopReason::NoWatcher));
        let wire: Observed =
            serde_json::from_str(r#"{"frontmost":"com.apple.Safari","armed":true}"#).unwrap();
        assert_eq!(verdict(&wire), None);
    }
}

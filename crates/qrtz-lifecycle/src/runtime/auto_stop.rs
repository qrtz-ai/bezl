//! Auto-stop of a recording whose meeting ended (Jared: a tab flip stopped a recording; Bozniak H on 5d695c9).
//!
//! The helper ends a meeting ~4 s after its tab or window loses focus, so `meeting.ended` alone never stops a
//! recording. Per recording, the watch keeps the set of live helper keys under the recorded meeting's identity
//! (the recorded key plus any new key detected under the same identity, e.g. the same calendar event in a new
//! tab). Only when the last of them ends does the stop arm, for [`GRACE_MS`]; the UI shows the countdown
//! (`note:auto-stop {state: "armed", deadlineAt}`) with a "keep recording" action.
//!
//! While armed, only a key already in the set that comes back under the same identity disarms it (a new key,
//! or the same `zoom-app:<pid>` key now naming a different meeting, never extends the grace), at most
//! [`MAX_REARMS`] times per recording; after that the stop runs at the grace end unless the user keeps
//! recording (which also resets the count).

use std::collections::HashSet;

use qrtz_model::EpochMs;

/// How long the meeting must stay gone before the recording stops (x `retry_scale` in tests).
pub const GRACE_MS: f64 = 3.0 * 60.0 * 1000.0;
/// Disarms by a returning meeting per recording before the grace always runs out.
pub const MAX_REARMS: u32 = 20;

#[derive(Debug)]
pub struct Watch {
    pub meeting_id: String,
    identity: u64,
    /// The identity is a calendar event (else the recorded helper key).
    calendar: bool,
    /// Every key that joined the set (recorded key included); live ones are in `live`.
    known: HashSet<String>,
    live: HashSet<String>,
    /// (generation, deadline) while armed.
    pub armed: Option<(u64, EpochMs)>,
    rearms: u32,
}

/// What the service must do after a watch update.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Arm (generation, deadline): start the timer and emit armed.
    Arm(u64, EpochMs),
    /// Disarm: abort the timer and emit cleared.
    Clear,
}

impl Watch {
    pub fn new(meeting_id: &str, recorded_key: &str, identity: u64, calendar: bool) -> Watch {
        Watch {
            meeting_id: meeting_id.to_string(),
            identity,
            calendar,
            known: HashSet::from([recorded_key.to_string()]),
            live: HashSet::from([recorded_key.to_string()]),
            armed: None,
            rearms: 0,
        }
    }

    /// A `meeting.detected` while recording, with the detection's identity hash (`calendar`: it is an event).
    /// Same identity: the recorded meeting. A different one is another meeting when both are calendar events
    /// (a reused `zoom-app:<pid>` key in the next event), or when the key isn't one of ours; a known key whose
    /// calendar slot ended (or began) since still counts as ours.
    pub fn detected(&mut self, key: &str, identity: u64, calendar: bool) -> Action {
        if identity != self.identity && ((calendar && self.calendar) || !self.known.contains(key)) {
            return Action::None;
        }
        if self.armed.is_some() {
            if !self.known.contains(key) || self.rearms >= MAX_REARMS {
                return Action::None;
            }
            self.rearms += 1;
            self.armed = None;
            self.live.insert(key.to_string());
            return Action::Clear;
        }
        self.known.insert(key.to_string());
        self.live.insert(key.to_string());
        Action::None
    }

    /// A `meeting.ended`: arms when the last live key of the recorded meeting is gone.
    pub fn ended(&mut self, key: &str, now: EpochMs, grace_ms: EpochMs, gen: u64) -> Action {
        if !self.live.remove(key) || !self.live.is_empty() || self.armed.is_some() {
            return Action::None;
        }
        let deadline = now + grace_ms;
        self.armed = Some((gen, deadline));
        Action::Arm(gen, deadline)
    }

    /// The user chose "keep recording".
    pub fn keep(&mut self) -> Action {
        self.rearms = 0;
        if self.armed.take().is_some() {
            Action::Clear
        } else {
            Action::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_change_then_the_new_key_ending_arms() {
        let mut w = Watch::new("m", "meet:a", 7, true);
        assert_eq!(w.detected("meet:b", 7, true), Action::None); // same calendar event, new tab
        assert_eq!(w.ended("meet:a", 0, 100, 1), Action::None, "meet:b still live");
        assert_eq!(w.ended("meet:b", 10, 100, 1), Action::Arm(1, 110));
    }

    #[test]
    fn only_a_known_key_of_the_same_identity_disarms() {
        let mut w = Watch::new("m", "zoom-app:1", 7, true);
        assert!(matches!(w.ended("zoom-app:1", 0, 100, 1), Action::Arm(..)));
        assert_eq!(
            w.detected("zoom-app:1", 9, true),
            Action::None,
            "same pid, another event"
        );
        assert_eq!(
            w.detected("zoom-app:2", 7, true),
            Action::None,
            "new key while armed"
        );
        assert_eq!(w.detected("zoom-app:1", 7, true), Action::Clear);
        // The event's slot ended: the same key now has no calendar identity, and still counts as ours.
        assert!(matches!(w.ended("zoom-app:1", 0, 100, 2), Action::Arm(..)));
        assert_eq!(
            w.detected("zoom-app:2", 5, false),
            Action::None,
            "unknown key, other identity"
        );
        assert_eq!(w.detected("zoom-app:1", 5, false), Action::Clear);
    }

    #[test]
    fn rearms_are_capped_and_keep_resets_them() {
        let mut w = Watch::new("m", "k", 7, false);
        for i in 0..MAX_REARMS {
            assert!(matches!(w.ended("k", 0, 100, i as u64), Action::Arm(..)));
            assert_eq!(w.detected("k", 7, false), Action::Clear);
        }
        assert!(matches!(w.ended("k", 0, 100, 99), Action::Arm(..)));
        assert_eq!(
            w.detected("k", 7, false),
            Action::None,
            "cap reached: the grace runs out"
        );
        assert_eq!(w.keep(), Action::Clear);
        assert_eq!(w.detected("k", 7, false), Action::None);
        assert!(matches!(w.ended("k", 0, 100, 100), Action::Arm(..)));
        assert_eq!(
            w.detected("k", 7, false),
            Action::Clear,
            "count reset by keep recording"
        );
    }
}

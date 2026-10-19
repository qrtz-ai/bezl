//! Once-per-meeting detection prompt (Jared: Meet/Zoom tab flips re-prompted on every `meeting.detected`).
//!
//! The helper ends a meeting after a short grace (tab switches, window re-creates), so the same meeting is
//! detected again and again. The prompt is keyed by meeting identity instead: the overlapping calendar event
//! (`calendar:<id>`) when there is one, else the helper's key (Meet code, Zoom id, app + window). An identity
//! counts as prompted from its first prompt (dismissed or declined included) until it has been gone for
//! [`GONE_MS`]. In memory only.

use std::collections::{HashMap, HashSet};

use qrtz_model::EpochMs;

/// How long a meeting must be gone before the same identity prompts again.
pub const GONE_MS: EpochMs = 5 * 60 * 1000;

#[derive(Debug)]
struct Entry {
    /// Helper keys currently live for this identity (calendar keys never end, so they are not tracked).
    live: HashSet<String>,
    /// Last detection, or when the last live key ended.
    last_ms: EpochMs,
}

#[derive(Debug, Default)]
pub struct PromptDedupe {
    entries: HashMap<String, Entry>,
    /// helper key -> identity, so `meeting.ended { key }` finds its entry.
    by_key: HashMap<String, String>,
}

impl PromptDedupe {
    /// The identity for a detection: the calendar event when one overlaps, else the helper key.
    pub fn identity(key: &str, calendar_event: Option<&str>) -> String {
        match calendar_event {
            Some(id) => format!("calendar:{id}"),
            None => key.to_string(),
        }
    }

    /// Records a detection; true when this identity has not been prompted (so prompt now).
    pub fn observe(&mut self, key: &str, identity: &str, now: EpochMs) -> bool {
        self.expire(now);
        let first = !self.entries.contains_key(identity);
        let e = self.entries.entry(identity.to_string()).or_insert_with(|| Entry {
            live: HashSet::new(),
            last_ms: now,
        });
        e.last_ms = now;
        if !key.starts_with("calendar:") {
            e.live.insert(key.to_string());
            self.by_key.insert(key.to_string(), identity.to_string());
        }
        first
    }

    /// The identity a live helper key was detected under.
    pub fn identity_of(&self, key: &str) -> Option<String> {
        self.by_key.get(key).cloned()
    }

    /// A helper key ended: its identity starts counting as gone once no key of it is live.
    pub fn ended(&mut self, key: &str, now: EpochMs) {
        if let Some(id) = self.by_key.remove(key) {
            if let Some(e) = self.entries.get_mut(&id) {
                e.live.remove(key);
                e.last_ms = now;
            }
        }
    }

    fn expire(&mut self, now: EpochMs) {
        self.entries
            .retain(|_, e| !e.live.is_empty() || now.saturating_sub(e.last_ms) <= GONE_MS);
        let entries = &self.entries;
        self.by_key.retain(|_, id| entries.contains_key(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: EpochMs = 60 * 1000;

    #[test]
    fn flapping_same_meeting_prompts_once() {
        let mut d = PromptDedupe::default();
        let mut prompts = 0;
        for i in 0..20 {
            let t = i * 10_000;
            if d.observe("meet:abc-defg-hij", "meet:abc-defg-hij", t) {
                prompts += 1;
            }
            d.ended("meet:abc-defg-hij", t + 5_000);
        }
        assert_eq!(prompts, 1);
    }

    #[test]
    fn different_meeting_prompts_again() {
        let mut d = PromptDedupe::default();
        assert!(d.observe("meet:aaa-bbbb-ccc", "meet:aaa-bbbb-ccc", 0));
        assert!(d.observe("zoom-web:123", "zoom-web:123", 1_000));
        assert!(!d.observe("meet:aaa-bbbb-ccc", "meet:aaa-bbbb-ccc", 2_000));
    }

    #[test]
    fn same_meeting_much_later_prompts_again() {
        let mut d = PromptDedupe::default();
        assert!(d.observe("meet:abc", "meet:abc", 0));
        d.ended("meet:abc", MIN);
        assert!(!d.observe("meet:abc", "meet:abc", 4 * MIN));
        d.ended("meet:abc", 5 * MIN);
        assert!(d.observe("meet:abc", "meet:abc", 11 * MIN));
    }

    #[test]
    fn live_meeting_never_expires() {
        let mut d = PromptDedupe::default();
        assert!(d.observe("zoom-app:1", "zoom-app:1", 0));
        // A second window of the same meeting flaps while the first stays live for an hour.
        assert!(!d.observe("zoom-app:1", "zoom-app:1", 60 * MIN));
    }

    #[test]
    fn calendar_start_and_ax_detection_share_one_prompt() {
        let mut d = PromptDedupe::default();
        let id = PromptDedupe::identity("calendar:ev1", Some("ev1"));
        assert!(d.observe("calendar:ev1", &id, 0));
        let ax = PromptDedupe::identity("meet:abc", Some("ev1"));
        assert!(!d.observe("meet:abc", &ax, MIN));
        d.ended("meet:abc", 2 * MIN);
        assert!(!d.observe("meet:abc", &ax, 3 * MIN));
        d.ended("meet:abc", 4 * MIN);
        // Gone for longer than GONE_MS: the same event prompts again.
        assert!(d.observe("calendar:ev1", &id, 30 * MIN));
        // A calendar start with no AX meeting expires GONE_MS after it fired.
        assert!(d.observe("calendar:ev2", "calendar:ev2", 0));
        assert!(!d.observe("calendar:ev2", "calendar:ev2", 4 * MIN));
        assert!(d.observe("calendar:ev2", "calendar:ev2", 10 * MIN));
    }
}

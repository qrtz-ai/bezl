//! Once-per-meeting detection prompt (Jared: Meet/Zoom tab flips re-prompted on every `meeting.detected`).
//!
//! The helper ends a meeting after a short grace (tab switches, window re-creates), so the same meeting is
//! detected again and again. The prompt is keyed by meeting identity instead: the overlapping calendar event
//! (`calendar:<id>`) when there is one, else the helper's key (Meet code, Zoom id, app + window). An identity
//! counts as prompted from its first prompt (dismissed or declined included) until it has been gone for
//! [`GONE_MS`]. In memory only, keys and identities stored as 64-bit hashes (no meeting codes kept), at most
//! [`CAP`] identities (the oldest goes), and cleared when the helper is lost or restarts (its live keys will
//! never see a `meeting.ended`).
//!
//! L1: a detection while recording (or while any capture runs) still counts: it uses up that meeting's one
//! prompt, so the meeting doesn't prompt again after the recording stops while it's still on.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};

use qrtz_model::EpochMs;

/// How long a meeting must be gone before the same identity prompts again.
pub const GONE_MS: EpochMs = 5 * 60 * 1000;
/// Most identities kept; the one seen longest ago is evicted first.
pub const CAP: usize = 64;

/// The calendar event id of a `calendar:<id>` key (shared by [`PromptDedupe::observe`] and the service).
pub fn calendar_event(key: &str) -> Option<&str> {
    key.strip_prefix("calendar:")
}

/// The stored hash of a key or identity.
pub fn hash(s: &str) -> u64 {
    BuildHasherDefault::<DefaultHasher>::default().hash_one(s)
}

#[derive(Debug)]
struct Entry {
    /// Helper keys currently live for this identity (calendar keys never end, so they are not tracked).
    live: HashSet<u64>,
    /// Last detection, or when the last live key ended.
    last_ms: EpochMs,
}

#[derive(Debug, Default)]
pub struct PromptDedupe {
    entries: HashMap<u64, Entry>,
    /// helper key -> identity (hashes), so `meeting.ended { key }` finds its entry.
    by_key: HashMap<u64, u64>,
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
        let (k, id) = (hash(key), hash(identity));
        let first = !self.entries.contains_key(&id);
        if first && self.entries.len() >= CAP {
            if let Some(old) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_ms)
                .map(|(i, _)| *i)
            {
                self.entries.remove(&old);
                self.by_key.retain(|_, i| *i != old);
            }
        }
        let e = self.entries.entry(id).or_insert_with(|| Entry {
            live: HashSet::new(),
            last_ms: now,
        });
        e.last_ms = now;
        if calendar_event(key).is_none() {
            e.live.insert(k);
            self.by_key.insert(k, id);
        }
        first
    }

    /// The identity hash a live helper key was detected under.
    pub fn identity_of(&self, key: &str) -> Option<u64> {
        self.by_key.get(&hash(key)).copied()
    }

    /// A helper key ended: its identity starts counting as gone once no key of it is live.
    pub fn ended(&mut self, key: &str, now: EpochMs) {
        let k = hash(key);
        if let Some(id) = self.by_key.remove(&k) {
            if let Some(e) = self.entries.get_mut(&id) {
                e.live.remove(&k);
                e.last_ms = now;
            }
        }
    }

    /// M1: the helper was lost or restarted; its live keys will never end, so forget everything.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.by_key.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
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

    #[test]
    fn clear_forgets_everything() {
        let mut d = PromptDedupe::default();
        assert!(d.observe("meet:abc", "meet:abc", 0));
        d.clear();
        assert!(d.is_empty());
        assert!(
            d.observe("meet:abc", "meet:abc", MIN),
            "after a helper restart the meeting may prompt again"
        );
    }

    #[test]
    fn cap_evicts_the_oldest() {
        let mut d = PromptDedupe::default();
        for i in 0..CAP as i64 {
            assert!(d.observe(&format!("zoom-app:{i}"), &format!("zoom-app:{i}"), i));
        }
        assert_eq!(d.len(), CAP);
        assert!(d.observe("zoom-app:new", "zoom-app:new", 1_000));
        assert_eq!(d.len(), CAP);
        assert!(
            d.observe("zoom-app:0", "zoom-app:0", 1_001),
            "the oldest was evicted"
        );
        assert!(!d.observe("zoom-app:new", "zoom-app:new", 1_002));
        assert!(!d.observe("zoom-app:63", "zoom-app:63", 1_003));
    }
}

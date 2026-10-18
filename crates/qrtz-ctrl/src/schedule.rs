//! Next-due math for ctrl schedules (slice 3, ADR 0105). Pure: a [`LocalZone`] turns local wall times into
//! UTC epoch ms (and back), so tests inject America/Los_Angeles or a fixed offset and never touch the system clock.
//!
//! DST for daily/weekly at a wall time:
//! - **Gap** (spring forward): the local time is skipped → run at the first valid local time after it.
//! - **Overlap** (fall back): the local time happens twice → run on the **first** occurrence.
//!
//! Catch-up: if the Mac slept or the app was quit past a due time, the caller fires **once** and then computes the
//! next due from the fire's wall time (so a daily does not burst every missed day).
use crate::task::Schedule;
use chrono::{DateTime, Datelike, Local, LocalResult, NaiveDate, TimeDelta, TimeZone, Timelike, Utc};

/// Local calendar parts at a UTC instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalParts {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// 0 = sunday … 6 = saturday (matches [`Schedule::Weekly`]).
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
}

/// Converts between UTC epoch ms and local wall-clock time. Injected so Linux tests can use a fixed zone.
pub trait LocalZone: Send + Sync {
    fn parts(&self, utc_ms: u64) -> LocalParts;
    /// Local Y-M-D H:M → UTC ms. Gap → first valid after; overlap → first occurrence.
    fn resolve_local(&self, year: i32, month: u32, day: u32, hour: u8, minute: u8) -> u64;
}

fn millis_utc<Tz: TimeZone>(dt: DateTime<Tz>) -> u64 {
    let n = dt.with_timezone(&Utc).timestamp_millis();
    if n < 0 {
        0
    } else {
        n as u64
    }
}

fn resolve_with<Tz: TimeZone>(tz: &Tz, year: i32, month: u32, day: u32, hour: u8, minute: u8) -> u64 {
    let naive = NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|d| d.and_hms_opt(hour as u32, minute as u32, 0))
        .expect("validated schedule times are real calendar dates");
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => millis_utc(dt),
        LocalResult::None => {
            // Spring-forward gap: walk forward a minute at a time until the wall time exists.
            let mut t = naive;
            for _ in 0..180 {
                t += TimeDelta::minutes(1);
                if let LocalResult::Single(dt) = tz.from_local_datetime(&t) {
                    return millis_utc(dt);
                }
            }
            // Should be unreachable for real zones (gaps are about an hour).
            millis_utc(tz.from_utc_datetime(&naive))
        }
        // Fall-back overlap: first (earlier) occurrence.
        LocalResult::Ambiguous(earliest, _) => millis_utc(earliest),
    }
}

fn parts_with<Tz: TimeZone>(tz: &Tz, utc_ms: u64) -> LocalParts {
    let dt = tz
        .timestamp_millis_opt(utc_ms as i64)
        .single()
        .unwrap_or_else(|| tz.timestamp_millis_opt(0).unwrap());
    LocalParts {
        year: dt.year(),
        month: dt.month(),
        day: dt.day(),
        weekday: dt.weekday().num_days_from_sunday() as u8,
        hour: dt.hour() as u8,
        minute: dt.minute() as u8,
    }
}

/// The host's local zone (`chrono::Local`).
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemLocalZone;

impl LocalZone for SystemLocalZone {
    fn parts(&self, utc_ms: u64) -> LocalParts {
        parts_with(&Local, utc_ms)
    }
    fn resolve_local(&self, year: i32, month: u32, day: u32, hour: u8, minute: u8) -> u64 {
        resolve_with(&Local, year, month, day, hour, minute)
    }
}

/// An IANA zone (tests: `America/Los_Angeles` for DST).
pub struct IanaZone(pub chrono_tz::Tz);

impl LocalZone for IanaZone {
    fn parts(&self, utc_ms: u64) -> LocalParts {
        parts_with(&self.0, utc_ms)
    }
    fn resolve_local(&self, year: i32, month: u32, day: u32, hour: u8, minute: u8) -> u64 {
        resolve_with(&self.0, year, month, day, hour, minute)
    }
}

fn add_days(year: i32, month: u32, day: u32, days: i64) -> (i32, u32, u32) {
    let d = NaiveDate::from_ymd_opt(year, month, day).expect("valid date") + TimeDelta::days(days);
    (d.year(), d.month(), d.day())
}

fn next_wall_after(after_ms: u64, hour: u8, minute: u8, weekday: Option<u8>, zone: &dyn LocalZone) -> u64 {
    let p = zone.parts(after_ms);
    for offset in 0..400 {
        let (y, m, d) = add_days(p.year, p.month, p.day, offset);
        if let Some(want) = weekday {
            let cand = zone.parts(zone.resolve_local(y, m, d, 12, 0));
            if cand.weekday != want {
                continue;
            }
        }
        let due = zone.resolve_local(y, m, d, hour, minute);
        if due > after_ms {
            return due;
        }
    }
    // Far future fallback (should not hit for real calendars).
    after_ms.saturating_add(86_400_000)
}

/// The next due instant for a task, or `None` if it will never run again (`once` after its first run).
///
/// `last_run_at` is the wall time of the last `run.recorded` (if any). Repeating schedules take the first wall
/// time **strictly after** that (or after `created_at - 1` when never run, so a task created at the due minute
/// is due immediately).
pub fn next_due(
    schedule: &Schedule,
    created_at: u64,
    last_run_at: Option<u64>,
    zone: &dyn LocalZone,
) -> Option<u64> {
    match *schedule {
        Schedule::Once { at: None } => {
            if last_run_at.is_some() {
                None
            } else {
                Some(created_at)
            }
        }
        Schedule::Once { at: Some(t) } => {
            if last_run_at.is_some() {
                None
            } else {
                Some(t)
            }
        }
        Schedule::Daily { hour, minute } => {
            let after = last_run_at.unwrap_or(created_at.saturating_sub(1));
            Some(next_wall_after(after, hour, minute, None, zone))
        }
        Schedule::Weekly {
            weekday,
            hour,
            minute,
        } => {
            let after = last_run_at.unwrap_or(created_at.saturating_sub(1));
            Some(next_wall_after(after, hour, minute, Some(weekday), zone))
        }
    }
}

/// True when the task should record a run at `now_ms` (due time ≤ now, and not finished as `once`).
pub fn is_due(
    schedule: &Schedule,
    created_at: u64,
    last_run_at: Option<u64>,
    now_ms: u64,
    zone: &dyn LocalZone,
) -> bool {
    next_due(schedule, created_at, last_run_at, zone).is_some_and(|d| d <= now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::Los_Angeles;

    fn la() -> IanaZone {
        IanaZone(Los_Angeles)
    }

    /// 2026-03-08 01:30 PST in LA → ms.
    fn ms(y: i32, m: u32, d: u32, h: u8, min: u8) -> u64 {
        la().resolve_local(y, m, d, h, min)
    }

    #[test]
    fn once_asap_is_due_at_creation_and_never_again() {
        let z = la();
        let created = ms(2026, 6, 1, 10, 0);
        assert_eq!(
            next_due(&Schedule::Once { at: None }, created, None, &z),
            Some(created)
        );
        assert!(is_due(&Schedule::Once { at: None }, created, None, created, &z));
        assert_eq!(
            next_due(&Schedule::Once { at: None }, created, Some(created), &z),
            None
        );
    }

    #[test]
    fn once_at_a_time_fires_once_even_if_missed() {
        let z = la();
        let at = ms(2026, 6, 1, 9, 0);
        let created = ms(2026, 6, 1, 8, 0);
        assert_eq!(
            next_due(&Schedule::Once { at: Some(at) }, created, None, &z),
            Some(at)
        );
        // Woke up at noon: still due once (catch-up).
        assert!(is_due(
            &Schedule::Once { at: Some(at) },
            created,
            None,
            ms(2026, 6, 1, 12, 0),
            &z
        ));
        assert_eq!(
            next_due(
                &Schedule::Once { at: Some(at) },
                created,
                Some(ms(2026, 6, 1, 12, 0)),
                &z
            ),
            None
        );
    }

    #[test]
    fn daily_next_is_today_or_tomorrow_in_local_time() {
        let z = la();
        let created = ms(2026, 6, 1, 8, 0); // Mon 08:00
        let daily = Schedule::Daily { hour: 9, minute: 30 };
        assert_eq!(next_due(&daily, created, None, &z), Some(ms(2026, 6, 1, 9, 30)));
        // After today's run, next is tomorrow.
        assert_eq!(
            next_due(&daily, created, Some(ms(2026, 6, 1, 9, 30)), &z),
            Some(ms(2026, 6, 2, 9, 30))
        );
        // Created after the wall time: first due is tomorrow.
        let late = ms(2026, 6, 1, 10, 0);
        assert_eq!(next_due(&daily, late, None, &z), Some(ms(2026, 6, 2, 9, 30)));
    }

    #[test]
    fn weekly_lands_on_the_right_weekday() {
        let z = la();
        // 2026-06-01 is a Monday (weekday 1). Weekly Sunday 18:00 → next is Jun 7.
        let created = ms(2026, 6, 1, 10, 0);
        let weekly = Schedule::Weekly {
            weekday: 0,
            hour: 18,
            minute: 0,
        };
        assert_eq!(next_due(&weekly, created, None, &z), Some(ms(2026, 6, 7, 18, 0)));
        assert_eq!(z.parts(ms(2026, 6, 7, 18, 0)).weekday, 0);
    }

    #[test]
    fn spring_forward_gap_runs_at_the_first_valid_time_after() {
        let z = la();
        // 2026-03-08: 02:00 PST → 03:00 PDT. Daily at 02:30 does not exist → 03:00 PDT.
        let due = z.resolve_local(2026, 3, 8, 2, 30);
        let parts = z.parts(due);
        assert_eq!((parts.hour, parts.minute), (3, 0), "gap lands on 03:00");
        // A task created before the gap is due at that resolved instant.
        let created = ms(2026, 3, 8, 1, 0);
        let daily = Schedule::Daily { hour: 2, minute: 30 };
        assert_eq!(next_due(&daily, created, None, &z), Some(due));
    }

    #[test]
    fn fall_back_overlap_runs_on_the_first_occurrence() {
        let z = la();
        // 2026-11-01: 01:30 happens twice (PDT then PST). First occurrence is the PDT one (UTC-7).
        let first = z.resolve_local(2026, 11, 1, 1, 30);
        // 01:30 PDT = 08:30 UTC; 01:30 PST = 09:30 UTC. First occurrence is the earlier.
        let pdt = Utc
            .with_ymd_and_hms(2026, 11, 1, 8, 30, 0)
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(first, pdt, "first occurrence is PDT");
        let created = ms(2026, 11, 1, 0, 30);
        let daily = Schedule::Daily { hour: 1, minute: 30 };
        assert_eq!(next_due(&daily, created, None, &z), Some(first));
    }

    #[test]
    fn catch_up_advances_from_now_not_every_missed_slot() {
        let z = la();
        let daily = Schedule::Daily { hour: 9, minute: 0 };
        let created = ms(2026, 6, 1, 8, 0); // Mon
                                            // Woke Thu 10:00 having never run: due (Mon 9 was missed).
        let now = ms(2026, 6, 4, 10, 0);
        assert!(is_due(&daily, created, None, now, &z));
        // After one catch-up fire at `now`, next is Fri 9:00 — not Tue/Wed/Thu.
        assert_eq!(
            next_due(&daily, created, Some(now), &z),
            Some(ms(2026, 6, 5, 9, 0))
        );
    }
}

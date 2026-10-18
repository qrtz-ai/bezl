//! Tasks and their schedules (slice 2, ADR 0103). A task is what Jared typed plus when it runs. The schedule is
//! local wall-clock time (hour, minute, weekday), never a UTC instant, so "every day at 9" stays 9 across DST
//! and time-zone changes; `once` is the exception: one instant (epoch ms), or `None` for "as soon as possible".
//! Computing the next due time against the local zone is the scheduler's job (slice 3).
use serde::{Deserialize, Serialize};

/// Longest task text, in characters (the input caps it too).
pub const TEXT_MAX: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Schedule {
    /// Run one time: at `at` (epoch ms), or as soon as possible when `None`.
    Once { at: Option<u64> },
    /// Every day at hour:minute, local time.
    Daily { hour: u8, minute: u8 },
    /// Every week on `weekday` (0 = sunday … 6 = saturday) at hour:minute, local time.
    Weekly { weekday: u8, hour: u8, minute: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    EmptyText,
    TextTooLong,
    BadTime,
    BadWeekday,
}

impl TaskError {
    pub fn message(&self) -> &'static str {
        match self {
            TaskError::EmptyText => "type what ctrl should do",
            TaskError::TextTooLong => "a task is at most 500 characters",
            TaskError::BadTime => "the time must be between 00:00 and 23:59",
            TaskError::BadWeekday => "the day must be sunday to saturday",
        }
    }
}

impl Schedule {
    pub fn validate(&self) -> Result<(), TaskError> {
        let time_ok = |h: u8, m: u8| h < 24 && m < 60;
        match *self {
            Schedule::Once { .. } => Ok(()),
            Schedule::Daily { hour, minute } if time_ok(hour, minute) => Ok(()),
            Schedule::Weekly { weekday, .. } if weekday > 6 => Err(TaskError::BadWeekday),
            Schedule::Weekly { hour, minute, .. } if time_ok(hour, minute) => Ok(()),
            _ => Err(TaskError::BadTime),
        }
    }

    /// Repeating schedules run until paused or deleted; `once` is done after its first run.
    pub fn repeats(&self) -> bool {
        !matches!(self, Schedule::Once { .. })
    }
}

/// Trim and check what was typed. Whitespace runs inside stay as typed (the decision layers read it).
pub fn clean_text(text: &str) -> Result<String, TaskError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(TaskError::EmptyText);
    }
    if t.chars().count() > TEXT_MAX {
        return Err(TaskError::TextTooLong);
    }
    Ok(t.to_string())
}

/// A task as the store's fold sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub text: String,
    pub schedule: Schedule,
    pub paused: bool,
    pub created_at: u64,
    /// Run ids, oldest first.
    pub runs: Vec<String>,
    /// Standing approvals of the confirm gate (slice 6), oldest first; revoking one removes it.
    #[serde(default)]
    pub approvals: Vec<crate::gate::Approval>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedules_validate_their_clock_fields() {
        assert!(Schedule::Once { at: None }.validate().is_ok());
        assert!(Schedule::Daily { hour: 23, minute: 59 }.validate().is_ok());
        assert_eq!(
            Schedule::Daily { hour: 24, minute: 0 }.validate(),
            Err(TaskError::BadTime)
        );
        assert_eq!(
            Schedule::Weekly {
                weekday: 7,
                hour: 9,
                minute: 0
            }
            .validate(),
            Err(TaskError::BadWeekday)
        );
        assert_eq!(
            Schedule::Weekly {
                weekday: 0,
                hour: 9,
                minute: 60
            }
            .validate(),
            Err(TaskError::BadTime)
        );
        assert!(!Schedule::Once { at: Some(1) }.repeats());
        assert!(Schedule::Weekly {
            weekday: 1,
            hour: 9,
            minute: 0
        }
        .repeats());
    }

    #[test]
    fn schedule_wire_shape_is_tagged_by_kind() {
        let w = Schedule::Weekly {
            weekday: 1,
            hour: 9,
            minute: 30,
        };
        assert_eq!(
            serde_json::to_value(w).unwrap(),
            serde_json::json!({"kind": "weekly", "weekday": 1, "hour": 9, "minute": 30})
        );
        let o: Schedule = serde_json::from_value(serde_json::json!({"kind": "once", "at": null})).unwrap();
        assert_eq!(o, Schedule::Once { at: None });
    }

    #[test]
    fn text_is_trimmed_and_bounded() {
        assert_eq!(
            clean_text("  clean up my downloads \n").unwrap(),
            "clean up my downloads"
        );
        assert_eq!(clean_text(" \t"), Err(TaskError::EmptyText));
        assert_eq!(clean_text(&"é".repeat(500)).unwrap().chars().count(), 500);
        assert_eq!(clean_text(&"a".repeat(501)), Err(TaskError::TextTooLong));
    }
}

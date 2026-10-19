//! nano's Apple Calendar cache (ADR 0216): the events EventKit gave the helper for one day back to 14 days ahead,
//! with only the fields nano shows and matches on, and the link from a recording to the event it was titled from.
//!
//! The cache is replaced whole on every read ([`CalendarRepo::replace`]), so a deleted or moved event never
//! lingers. Everything stored is re-checked here even though the helper already did (titles are third-party
//! text: control and tag characters are dropped and the length capped; a meeting link is kept only for a known
//! video-call host over https).
use qrtz_model::EpochMs;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::row::{i64_, opt_string, string};
use crate::Result;

/// How far back the window reaches (one day).
pub const WINDOW_BACK_MS: i64 = 24 * 3_600_000;
/// How far ahead the window reaches (14 days).
pub const WINDOW_AHEAD_MS: i64 = 14 * 24 * 3_600_000;
/// An event counts as "now" from this long before its start (joining a call a little early).
pub const EARLY_MS: i64 = 5 * 60_000;
/// Longest stored title, in chars.
pub const TITLE_MAX: usize = 200;
/// Most events kept from one read (a pathological calendar can't fill the database).
pub const EVENTS_MAX: usize = 2_000;

/// One calendar the user can pick in nano's settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarInfo {
    pub id: String,
    pub title: String,
}

/// One cached event: only these fields are ever stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub id: String,
    pub calendar_id: String,
    pub title: String,
    pub start_at: EpochMs,
    pub end_at: EpochMs,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub attendee_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_url: Option<String>,
}

/// Drops control, format (tag, bidi) and private-use characters, collapses whitespace, and caps the length.
pub fn clean_title(s: &str) -> String {
    let kept: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| {
            let u = *c as u32;
            !(c.is_control()
                || (0xE0000..=0xE007F).contains(&u) // tag characters
                || (0x200B..=0x200F).contains(&u)
                || (0x202A..=0x202E).contains(&u)
                || (0x2066..=0x2069).contains(&u)
                || u == 0xFEFF
                || (0xE000..=0xF8FF).contains(&u))
        })
        .collect();
    let joined = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(TITLE_MAX).collect()
}

/// Video-call hosts whose links nano keeps (https only). Anything else is dropped, never opened.
const MEETING_HOSTS: &[&str] = &[
    "zoom.us",
    "meet.google.com",
    "teams.microsoft.com",
    "teams.live.com",
    "webex.com",
    "whereby.com",
    "facetime.apple.com",
];

/// The link's host when it is an https link to a known video-call host (`zoom.us` covers `us02web.zoom.us`).
pub fn meeting_host(url: &str) -> Option<&'static str> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let host = authority.split(':').next()?.to_ascii_lowercase();
    MEETING_HOSTS
        .iter()
        .find(|h| host == **h || host.ends_with(&format!(".{h}")))
        .copied()
}

/// The meeting link to store: an allowed https link, without its fragment, at most 500 chars; else none.
pub fn clean_meeting_url(url: Option<&str>) -> Option<String> {
    let u = url?.trim();
    if u.len() > 500 || u.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    meeting_host(u)?;
    Some(u.split('#').next().unwrap_or(u).to_string())
}

/// The platform a link's host belongs to, in `MeetingPlatform` names (`zoom`, `meet`, `teams`).
fn link_platform(url: &str) -> Option<&'static str> {
    match meeting_host(url)? {
        "zoom.us" => Some("zoom"),
        "meet.google.com" => Some("meet"),
        "teams.microsoft.com" | "teams.live.com" => Some("teams"),
        _ => None,
    }
}

/// Is `calendar_id` among the chosen calendars (`None` = all of them)?
pub fn calendar_chosen(chosen: Option<&[String]>, calendar_id: &str) -> bool {
    chosen.is_none_or(|c| c.iter().any(|x| x == calendar_id))
}

/// The event a recording starting at `now` belongs to: a timed event (never all-day) on a chosen calendar that
/// runs at `now` or starts within [`EARLY_MS`]. With several: the one whose meeting link matches the detected
/// `platform`, else the one whose start is nearest `now`, else the earliest id (stable).
pub fn pick_event<'a>(
    events: &'a [CalendarEvent],
    now: EpochMs,
    platform: Option<&str>,
    chosen: Option<&[String]>,
) -> Option<&'a CalendarEvent> {
    let mut c: Vec<&CalendarEvent> = events
        .iter()
        .filter(|e| {
            !e.all_day
                && e.end_at > e.start_at
                && e.start_at - EARLY_MS <= now
                && now < e.end_at
                && calendar_chosen(chosen, &e.calendar_id)
        })
        .collect();
    let matches_platform = |e: &CalendarEvent| {
        platform.is_some_and(|p| e.meeting_url.as_deref().and_then(link_platform) == Some(p))
    };
    c.sort_by(|a, b| {
        matches_platform(b)
            .cmp(&matches_platform(a))
            .then((a.start_at - now).abs().cmp(&(b.start_at - now).abs()))
            .then(a.id.cmp(&b.id))
    });
    c.into_iter().next()
}

pub struct CalendarRepo<'a> {
    conn: &'a Connection,
}

impl<'a> CalendarRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        CalendarRepo { conn }
    }

    /// Replaces the whole cache with one read: the calendars, and the events (cleaned, deduplicated by id, capped
    /// at [`EVENTS_MAX`] earliest first). One transaction: a reader never sees half a read.
    pub fn replace(&self, calendars: &[CalendarInfo], events: &[CalendarEvent]) -> Result<usize> {
        let mut evs: Vec<&CalendarEvent> = events
            .iter()
            .filter(|e| !e.id.is_empty() && e.id.len() <= 512 && e.end_at >= e.start_at)
            .collect();
        evs.sort_by(|a, b| a.start_at.cmp(&b.start_at).then(a.id.cmp(&b.id)));
        evs.dedup_by(|a, b| a.id == b.id);
        evs.truncate(EVENTS_MAX);
        self.conn.execute_batch("SAVEPOINT calendar_replace")?;
        let r = (|| -> Result<usize> {
            self.conn
                .execute_batch("DELETE FROM calendar_events; DELETE FROM calendar_calendars;")?;
            let mut ci = self
                .conn
                .prepare_cached("INSERT OR REPLACE INTO calendar_calendars (id,title) VALUES (?,?)")?;
            for c in calendars.iter().filter(|c| !c.id.is_empty() && c.id.len() <= 512) {
                ci.execute(params![c.id, clean_title(&c.title)])?;
            }
            let mut ei = self.conn.prepare_cached(
                "INSERT OR REPLACE INTO calendar_events
                 (id,calendar_id,title,start_at,end_at,all_day,attendee_count,meeting_url) VALUES (?,?,?,?,?,?,?,?)",
            )?;
            for e in &evs {
                ei.execute(params![
                    e.id,
                    e.calendar_id,
                    clean_title(&e.title),
                    e.start_at,
                    e.end_at,
                    e.all_day,
                    e.attendee_count.min(10_000),
                    clean_meeting_url(e.meeting_url.as_deref()),
                ])?;
            }
            Ok(evs.len())
        })();
        match r {
            Ok(n) => {
                self.conn.execute_batch("RELEASE calendar_replace")?;
                Ok(n)
            }
            Err(e) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO calendar_replace; RELEASE calendar_replace");
                Err(e)
            }
        }
    }

    /// Wipes the cache (calendar turned off, or access revoked). Notes keep their titles and links.
    pub fn clear(&self) -> Result<()> {
        self.conn
            .execute_batch("DELETE FROM calendar_events; DELETE FROM calendar_calendars;")?;
        Ok(())
    }

    pub fn calendars(&self) -> Result<Vec<CalendarInfo>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT id,title FROM calendar_calendars ORDER BY title COLLATE NOCASE, id")?;
        let mut rows = st.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(CalendarInfo {
                id: string(r, "id")?,
                title: string(r, "title")?,
            });
        }
        Ok(out)
    }

    /// Every cached event that hasn't ended by `from`, earliest first.
    pub fn events_from(&self, from: EpochMs) -> Result<Vec<CalendarEvent>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT * FROM calendar_events WHERE end_at > ? ORDER BY start_at, id")?;
        let mut rows = st.query([from])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(to_event(r)?);
        }
        Ok(out)
    }

    /// The next `limit` timed events on the chosen calendars that haven't ended at `now`, earliest first.
    pub fn upcoming(
        &self,
        now: EpochMs,
        limit: usize,
        chosen: Option<&[String]>,
    ) -> Result<Vec<CalendarEvent>> {
        Ok(self
            .events_from(now)?
            .into_iter()
            .filter(|e| !e.all_day && calendar_chosen(chosen, &e.calendar_id))
            .take(limit)
            .collect())
    }

    /// Links a recording to the event it was titled from.
    pub fn link_meeting(&self, meeting_id: &str, event_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .prepare_cached("UPDATE meetings SET calendar_event_id=? WHERE id=?")?
            .execute(params![event_id, meeting_id])?
            > 0)
    }

    pub fn meeting_event(&self, meeting_id: &str) -> Result<Option<String>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT calendar_event_id FROM meetings WHERE id=?")?;
        let mut rows = st.query([meeting_id])?;
        match rows.next()? {
            Some(r) => Ok(opt_string(r, "calendar_event_id")?),
            None => Ok(None),
        }
    }
}

fn to_event(r: &rusqlite::Row) -> Result<CalendarEvent> {
    Ok(CalendarEvent {
        id: string(r, "id")?,
        calendar_id: string(r, "calendar_id")?,
        title: string(r, "title")?,
        start_at: i64_(r, "start_at")?,
        end_at: i64_(r, "end_at")?,
        all_day: i64_(r, "all_day")? != 0,
        attendee_count: i64_(r, "attendee_count")?.clamp(0, u32::MAX as i64) as u32,
        meeting_url: opt_string(r, "meeting_url")?,
    })
}

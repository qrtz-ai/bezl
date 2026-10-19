//! nano's Apple Calendar (ADR 0217): the host side of the helper's EventKit reader. Fully offline: the events come
//! from the calendar database on this Mac through nano's helper (`calendar.*` lines), nothing is fetched.
//!
//! - **Access** is asked for only by [`NanoCalendar::connect`] (`calendar:connect`: the user turned the calendar
//!   on in nano or pressed 'connect calendar'). Reads and checks never prompt.
//! - **Sync**: while `note.calendar.enabled`, the window one day back to 14 days ahead is read on start, on every
//!   `calendar.changed` (EKEventStoreChanged), on a settings change and every 10 minutes, and replaces the cache
//!   (qrtz-storage `calendar_events`). Off, or access gone: the cache is wiped.
//! - **Recording**: [`qrtz_lifecycle::runtime::CalendarLookup`] names a recording after the overlapping event and
//!   links it (`meetings.calendar_event_id`); at an event's start the existing detected-meeting prompt is raised
//!   (same `detectPrompt` / auto-record rules as an AX detection).
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qrtz_helper_bridge::{encode_host_message, parse_helper_line, Supervisor};
use qrtz_storage::calendar::{calendar_chosen, link_platform, pick_event, WINDOW_AHEAD_MS, WINDOW_BACK_MS};
use qrtz_storage::{CalendarEvent, CalendarInfo, StorageHandle};
use serde_json::{json, Value};

use crate::service::{Events, SharedConfig};

/// The periodic re-read.
pub const REFRESH_EVERY: Duration = Duration::from_secs(600);
/// How often the start-of-meeting prompt and the settings are looked at.
pub const TICK: Duration = Duration::from_secs(30);
/// An event "starts now" within this of its start (the prompt fires once per event).
pub const PROMPT_SLACK_MS: i64 = 60_000;
/// How long a `calendar.*` request waits for the helper's answer (the access prompt waits on the user).
pub const ANSWER_WAIT: Duration = Duration::from_secs(10);
pub const CONNECT_WAIT: Duration = Duration::from_secs(300);
/// Events the nano page's 'upcoming' block shows at most.
pub const UPCOMING_MAX: usize = 20;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `note.calendar` from the merged config.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CalendarSettings {
    pub enabled: bool,
    /// `None` = all calendars.
    pub chosen: Option<Vec<String>>,
}

impl CalendarSettings {
    pub fn from_config(cfg: &Value) -> Self {
        let c = &cfg["note"]["calendar"];
        CalendarSettings {
            enabled: c["enabled"].as_bool().unwrap_or(false),
            chosen: c["calendars"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()),
        }
    }
}

/// The `meeting.detected` an event's start raises (key `calendar:<id>`, so it never matches an AX meeting's end).
pub fn detected_line(e: &CalendarEvent) -> Value {
    let platform = e
        .meeting_url
        .as_deref()
        .and_then(link_platform)
        .unwrap_or("other");
    json!({"type": "meeting.detected", "key": format!("calendar:{}", e.id), "title": e.title,
           "platform": platform, "app": "Calendar"})
}

/// The events whose start is due for the prompt at `now` and not yet prompted (marks them).
pub fn due_prompts(
    events: &[CalendarEvent],
    now: i64,
    chosen: Option<&[String]>,
    prompted: &mut HashSet<String>,
) -> Vec<CalendarEvent> {
    events
        .iter()
        .filter(|e| {
            !e.all_day
                && calendar_chosen(chosen, &e.calendar_id)
                && (e.start_at - now).abs() <= PROMPT_SLACK_MS
                && now < e.end_at
        })
        .filter(|e| prompted.insert(e.id.clone()))
        .cloned()
        .collect()
}

/// Raises a detected-meeting prompt from a `meeting.detected` line.
pub type PromptFn = Arc<dyn Fn(Value) + Send + Sync>;

#[derive(Default)]
struct State {
    access: String,
    events: Vec<CalendarEvent>,
    calendars: Vec<CalendarInfo>,
    last_sync_at: Option<i64>,
    prompted: HashSet<String>,
    seen: Option<CalendarSettings>,
}

pub struct NanoCalendar {
    db: StorageHandle,
    sup: Option<Supervisor>,
    config: SharedConfig,
    events: Events,
    state: Mutex<State>,
    kick: tokio::sync::Notify,
    /// Raises the detected-meeting prompt (the service feeds the line to the runtime).
    prompt: Mutex<Option<PromptFn>>,
}

impl NanoCalendar {
    pub fn new(
        db: StorageHandle,
        sup: Option<Supervisor>,
        config: SharedConfig,
        events: Events,
    ) -> Arc<Self> {
        Arc::new(NanoCalendar {
            db,
            sup,
            config,
            events,
            state: Mutex::new(State {
                access: "not-determined".into(),
                ..Default::default()
            }),
            kick: Default::default(),
            prompt: Mutex::new(None),
        })
    }

    pub fn set_prompt(&self, f: PromptFn) {
        *self.prompt.lock().unwrap() = Some(f);
    }

    fn settings(&self) -> CalendarSettings {
        CalendarSettings::from_config(&self.config.get())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// One request to nano's helper and its answer (same id, a `calendar.*` type), or `None` (no helper, timeout,
    /// an older helper that doesn't know the message).
    async fn ask(&self, msg: Value, wait: Duration) -> Option<Value> {
        let sup = self.sup.as_ref()?;
        let id = msg["id"].as_str()?.to_owned();
        let line = encode_host_message(&msg).ok()?;
        let mut lines = sup.subscribe_lines();
        if !sup.send(line).await {
            return None;
        }
        tokio::time::timeout(wait, async {
            loop {
                match lines.recv().await {
                    Ok(l) if l.contains("\"calendar.") => {
                        let Ok(v) = parse_helper_line(&l) else { continue };
                        if v["id"].as_str() == Some(id.as_str()) {
                            return Some(v);
                        }
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return None,
                }
            }
        })
        .await
        .ok()
        .flatten()
    }

    fn set_access(&self, st: &str) {
        let changed = {
            let mut s = self.lock();
            let c = s.access != st;
            s.access = st.to_owned();
            c
        };
        if changed {
            self.emit_changed();
        }
    }

    fn emit_changed(&self) {
        self.events.emit("calendar:changed", self.status());
    }

    /// `calendar:status`.
    pub fn status(&self) -> Value {
        let set = self.settings();
        let s = self.lock();
        json!({
            "enabled": set.enabled,
            "access": s.access,
            "calendars": s.calendars,
            "chosen": set.chosen,
            "lastSyncAt": s.last_sync_at,
        })
    }

    /// `calendar:upcoming`: the next timed events on the chosen calendars.
    pub fn upcoming(&self, limit: usize) -> Value {
        let set = self.settings();
        if !set.enabled {
            return json!({"events": []});
        }
        let now = now_ms();
        let s = self.lock();
        let evs: Vec<&CalendarEvent> = s
            .events
            .iter()
            .filter(|e| {
                e.end_at > now && !e.all_day && calendar_chosen(set.chosen.as_deref(), &e.calendar_id)
            })
            .take(limit.min(UPCOMING_MAX))
            .collect();
        json!({"events": evs})
    }

    /// `calendar:check`: the access state, without asking.
    pub async fn check(&self) -> Value {
        let id = crate::wiring::uuid_v4();
        if let Some(v) = self
            .ask(json!({"type": "calendar.check", "id": id}), ANSWER_WAIT)
            .await
        {
            if let Some(st) = v["state"].as_str() {
                self.set_access(st);
            }
        } else if self.sup.is_none() {
            self.set_access("unsupported");
        }
        self.status()
    }

    /// `calendar:connect`: ask macOS for access (the prompt shows once), and on a grant turn the calendar on and
    /// read it. The only path that ever prompts.
    pub async fn connect(&self) -> Value {
        let id = crate::wiring::uuid_v4();
        let st = match self
            .ask(json!({"type": "calendar.request", "id": id}), CONNECT_WAIT)
            .await
        {
            Some(v) => v["state"].as_str().unwrap_or("denied").to_owned(),
            None => "unsupported".to_owned(),
        };
        self.set_access(&st);
        if st == "granted" {
            if !self.settings().enabled {
                let r = self
                    .config
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set(&json!({"note": {"calendar": {"enabled": true}}}));
                match r {
                    Ok(next) => self.events.emit("config:changed", next),
                    Err(e) => tracing::warn!(error = %e, "calendar: enabling failed"),
                }
            }
            self.refresh().await;
        }
        self.status()
    }

    /// Re-reads the window (or wipes the cache when the calendar is off or access is gone).
    pub async fn refresh(&self) {
        let set = self.settings();
        if !set.enabled {
            return self.wipe().await;
        }
        let now = now_ms();
        let id = crate::wiring::uuid_v4();
        let ask = json!({"type": "calendar.read", "id": id, "fromMs": now - WINDOW_BACK_MS, "toMs": now + WINDOW_AHEAD_MS});
        let Some(v) = self.ask(ask, ANSWER_WAIT).await else {
            return; // no helper right now: keep what we have, the next tick retries
        };
        match v["type"].as_str() {
            Some("calendar.events") => {
                let cals: Vec<CalendarInfo> =
                    serde_json::from_value(v["calendars"].clone()).unwrap_or_default();
                let evs: Vec<CalendarEvent> = v["events"]
                    .as_array()
                    .map(|a| a.iter().filter_map(event_from_wire).collect())
                    .unwrap_or_default();
                let stored = self
                    .db
                    .call(move |db| {
                        let c = db.calendar();
                        c.replace(&cals, &evs)?;
                        Ok((c.calendars()?, c.events_from(now - WINDOW_BACK_MS)?))
                    })
                    .await;
                match stored {
                    Ok((cals, evs)) => {
                        {
                            let mut s = self.lock();
                            s.access = "granted".into();
                            s.calendars = cals;
                            s.events = evs;
                            s.last_sync_at = Some(now);
                        }
                        self.emit_changed();
                    }
                    Err(e) => tracing::warn!(error = %e, "calendar: cache write failed"),
                }
            }
            Some("calendar.access") => {
                let st = v["state"].as_str().unwrap_or("denied").to_owned();
                self.set_access(&st);
                self.wipe().await;
            }
            _ => {}
        }
    }

    async fn wipe(&self) {
        {
            let mut s = self.lock();
            s.events.clear();
            s.calendars.clear();
        }
        if let Err(e) = self.db.call(|db| db.calendar().clear()).await {
            tracing::warn!(error = %e, "calendar: cache wipe failed");
        }
        // Also the answer to turning it off: the views re-read on/off from this.
        self.emit_changed();
    }

    /// The sync loop: loads the cache, then re-reads on `calendar.changed`, settings changes, a kick and every
    /// [`REFRESH_EVERY`]; raises the start-of-meeting prompt every [`TICK`]. Call once, inside tokio.
    pub fn spawn(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            // What the last run cached, until the first read answers.
            if let Ok((cals, evs)) = me
                .db
                .call(|db| {
                    let c = db.calendar();
                    // M2: a cache from before links lost their query keeps no passcode at rest.
                    c.scrub_links()?;
                    Ok((c.calendars()?, c.events_from(now_ms() - WINDOW_BACK_MS)?))
                })
                .await
            {
                let mut s = me.lock();
                s.calendars = cals;
                s.events = evs;
            }
            let mut changed = me.sup.as_ref().map(|s| s.subscribe_lines());
            let mut last_read = tokio::time::Instant::now() - REFRESH_EVERY;
            let mut tick = tokio::time::interval(TICK);
            loop {
                let mut read = false;
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = me.kick.notified() => read = true,
                    l = async {
                        match changed.as_mut() {
                            Some(r) => r.recv().await.ok(),
                            None => std::future::pending().await,
                        }
                    } => {
                        let Some(l) = l else { changed = None; continue };
                        // A new helper process (hello) or a database change: read again.
                        if l.contains("\"calendar.changed\"") || l.contains("\"type\":\"hello\"") {
                            read = true;
                        } else if l.contains("\"calendar.access\"") && !l.contains("\"id\"") {
                            if let Ok(v) = parse_helper_line(&l) {
                                me.set_access(v["state"].as_str().unwrap_or("denied"));
                            }
                            read = true;
                        } else {
                            continue;
                        }
                    }
                }
                let set = me.settings();
                {
                    let mut s = me.lock();
                    if s.seen.as_ref() != Some(&set) {
                        s.seen = Some(set.clone());
                        read = true;
                    }
                }
                if read || last_read.elapsed() >= REFRESH_EVERY {
                    last_read = tokio::time::Instant::now();
                    me.refresh().await;
                }
                if set.enabled {
                    let due = {
                        let mut s = me.lock();
                        let evs = s.events.clone();
                        due_prompts(&evs, now_ms(), set.chosen.as_deref(), &mut s.prompted)
                    };
                    let f = me.prompt.lock().unwrap().clone();
                    if let Some(f) = f {
                        for e in due {
                            f(detected_line(&e));
                        }
                    }
                }
            }
        });
    }

    /// Re-read soon (the settings changed).
    pub fn kick(&self) {
        self.kick.notify_one();
    }
}

/// A `calendar.events` entry as the cache stores it (the storage layer cleans it again).
fn event_from_wire(v: &Value) -> Option<CalendarEvent> {
    Some(CalendarEvent {
        id: v["id"].as_str()?.to_owned(),
        calendar_id: v["calendarId"].as_str()?.to_owned(),
        title: v["title"].as_str()?.to_owned(),
        start_at: v["startMs"].as_i64()?,
        end_at: v["endMs"].as_i64()?,
        all_day: v["allDay"].as_bool().unwrap_or(false),
        attendee_count: v["attendees"].as_u64().unwrap_or(0).min(10_000) as u32,
        // Defensive (M2): an older or broken helper's link still loses its query/fragment before the cache.
        meeting_url: qrtz_storage::calendar::clean_meeting_url(v["meetingUrl"].as_str()),
    })
}

/// The runtime is built before the cache's database handle exists; this is set right after (`build_app`).
#[derive(Default)]
pub struct LateCalendar(pub std::sync::OnceLock<Arc<NanoCalendar>>);

impl qrtz_lifecycle::runtime::CalendarLookup for LateCalendar {
    fn event_at(&self, now: i64, platform: Option<&str>) -> Option<(String, String)> {
        self.0.get()?.event_at(now, platform)
    }
    fn link(&self, meeting_id: &str, event_id: &str) {
        if let Some(c) = self.0.get() {
            c.link(meeting_id, event_id)
        }
    }
}

impl qrtz_lifecycle::runtime::CalendarLookup for NanoCalendar {
    fn event_at(&self, now: i64, platform: Option<&str>) -> Option<(String, String)> {
        let set = self.settings();
        if !set.enabled {
            return None;
        }
        let s = self.lock();
        pick_event(&s.events, now, platform, set.chosen.as_deref()).map(|e| (e.id.clone(), e.title.clone()))
    }

    fn link(&self, meeting_id: &str, event_id: &str) {
        let (db, m, e) = (self.db.clone(), meeting_id.to_owned(), event_id.to_owned());
        if let Ok(h) = tokio::runtime::Handle::try_current() {
            h.spawn(async move {
                if let Err(err) = db.call(move |db| db.calendar().link_meeting(&m, &e)).await {
                    tracing::warn!(error = %err, "calendar: link failed");
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, start: i64, end: i64) -> CalendarEvent {
        CalendarEvent {
            id: id.into(),
            calendar_id: "w".into(),
            title: format!("t {id}"),
            start_at: start,
            end_at: end,
            all_day: false,
            attendee_count: 1,
            meeting_url: None,
        }
    }

    #[test]
    fn settings_default_off_and_all_calendars() {
        assert_eq!(
            CalendarSettings::from_config(&json!({})),
            CalendarSettings::default()
        );
        let s = CalendarSettings::from_config(
            &json!({"note": {"calendar": {"enabled": true, "calendars": ["a"]}}}),
        );
        assert!(s.enabled);
        assert_eq!(s.chosen.as_deref(), Some(&["a".to_string()][..]));
        let all = CalendarSettings::from_config(
            &json!({"note": {"calendar": {"enabled": true, "calendars": null}}}),
        );
        assert_eq!(all.chosen, None);
    }

    #[test]
    fn prompts_fire_once_at_the_start() {
        let mut seen = HashSet::new();
        let evs = vec![ev("now", 1_000, 5_000_000), ev("later", 10 * 60_000, 20 * 60_000)];
        let due = due_prompts(&evs, 30_000, None, &mut seen);
        assert_eq!(due.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["now"]);
        assert!(
            due_prompts(&evs, 40_000, None, &mut seen).is_empty(),
            "once per event"
        );
        assert!(
            due_prompts(&evs, 30_000, Some(&[]), &mut HashSet::new()).is_empty(),
            "unchosen calendar"
        );
    }

    #[test]
    fn detected_line_parses_as_a_meeting_detection() {
        let mut e = ev("x", 0, 1);
        e.meeting_url = Some("https://meet.google.com/abc".into());
        let l = detected_line(&e);
        assert_eq!(l["platform"], "google-meet");
        assert_eq!(l["key"], "calendar:x");
        let parsed: qrtz_lifecycle::runtime::HelperInput = serde_json::from_value(l).unwrap();
        assert!(matches!(
            parsed,
            qrtz_lifecycle::runtime::HelperInput::MeetingDetected { .. }
        ));
        assert_eq!(detected_line(&ev("y", 0, 1))["platform"], "other");
    }
}

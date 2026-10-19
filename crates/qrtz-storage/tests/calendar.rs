//! ADR 0217: nano's Apple Calendar cache and the recording link.
use qrtz_storage::calendar::{clean_meeting_url, clean_title, pick_event, EARLY_MS, EVENTS_MAX, TITLE_MAX};
use qrtz_storage::{CalendarEvent, CalendarInfo, Db};

const H: i64 = 3_600_000;
const NOW: i64 = 1_790_000_000_000;

fn ev(id: &str, cal: &str, start: i64, end: i64) -> CalendarEvent {
    CalendarEvent {
        id: id.into(),
        calendar_id: cal.into(),
        title: format!("title {id}"),
        start_at: start,
        end_at: end,
        all_day: false,
        attendee_count: 3,
        meeting_url: None,
    }
}

fn db() -> Db {
    Db::open_in_memory().unwrap()
}

fn cals() -> Vec<CalendarInfo> {
    vec![
        CalendarInfo {
            id: "w".into(),
            title: "Work".into(),
        },
        CalendarInfo {
            id: "h".into(),
            title: "home".into(),
        },
    ]
}

#[test]
fn replace_swaps_the_whole_window_and_keeps_minimal_fields() {
    let db = db();
    let c = db.calendar();
    let mut a = ev("a", "w", NOW + H, NOW + 2 * H);
    a.meeting_url = Some("https://us02web.zoom.us/j/123?pwd=x#frag".into());
    a.title = "weekly\u{0007} sync\u{E0041}\n\u{202E}".into();
    assert_eq!(
        c.replace(&cals(), &[a, ev("b", "h", NOW + 3 * H, NOW + 4 * H)])
            .unwrap(),
        2
    );
    let got = c.events_from(NOW).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].title, "weekly sync");
    assert_eq!(
        got[0].meeting_url.as_deref(),
        Some("https://us02web.zoom.us/j/123?pwd=x")
    );
    assert_eq!(got[0].attendee_count, 3);
    assert_eq!(
        c.calendars()
            .unwrap()
            .iter()
            .map(|x| x.title.as_str())
            .collect::<Vec<_>>(),
        ["home", "Work"]
    );
    // The next read drops what's gone.
    c.replace(&cals()[..1], &[ev("c", "w", NOW, NOW + H)]).unwrap();
    let ids: Vec<_> = c
        .events_from(NOW - H)
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, ["c"]);
    assert_eq!(c.calendars().unwrap().len(), 1);
    // The schema has no column for anything else.
    let cols: Vec<String> = db
        .connection()
        .prepare("SELECT name FROM pragma_table_info('calendar_events')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        cols,
        [
            "id",
            "calendar_id",
            "title",
            "start_at",
            "end_at",
            "all_day",
            "attendee_count",
            "meeting_url"
        ]
    );
}

#[test]
fn replace_dedups_caps_and_drops_bad_rows() {
    let db = db();
    let mut evs: Vec<_> = (0..EVENTS_MAX + 10)
        .map(|i| ev(&format!("e{i:05}"), "w", NOW + i as i64, NOW + H))
        .collect();
    evs.push(ev("e00000", "w", NOW, NOW + H)); // duplicate id
    evs.push(ev("", "w", NOW, NOW + H)); // no id
    evs.push(ev("backwards", "w", NOW + H, NOW)); // ends before it starts
    assert_eq!(db.calendar().replace(&cals(), &evs).unwrap(), EVENTS_MAX);
    let all = db.calendar().events_from(0).unwrap();
    assert_eq!(all.len(), EVENTS_MAX);
    assert!(all.iter().all(|e| !e.id.is_empty() && e.id != "backwards"));
}

#[test]
fn titles_and_links_are_cleaned() {
    let long = "x".repeat(TITLE_MAX + 50);
    assert_eq!(clean_title(&long).chars().count(), TITLE_MAX);
    assert_eq!(clean_title("  a\t\tb  "), "a b");
    for bad in [
        "http://zoom.us/j/1",
        "https://evil.example/j/1",
        "https://zoom.us.evil.example/j",
        "https://user@zoom.us/j/1",
        "javascript:alert(1)",
        "https://meet.google.com/a b",
    ] {
        assert_eq!(clean_meeting_url(Some(bad)), None, "{bad}");
    }
    for ok in [
        "https://meet.google.com/abc-defg-hij",
        "https://teams.microsoft.com/l/meetup-join/x",
        "https://zoom.us:443/j/9",
    ] {
        assert!(clean_meeting_url(Some(ok)).is_some(), "{ok}");
    }
}

#[test]
fn pick_event_prefers_platform_then_nearest_start() {
    let mut zoom = ev("z", "w", NOW - 30 * 60_000, NOW + H);
    zoom.meeting_url = Some("https://zoom.us/j/1".into());
    let mut meet = ev("m", "w", NOW, NOW + H);
    meet.meeting_url = Some("https://meet.google.com/abc".into());
    let evs = vec![zoom, meet, ev("soon", "h", NOW + EARLY_MS - 1, NOW + H)];
    assert_eq!(pick_event(&evs, NOW, Some("zoom"), None).unwrap().id, "z");
    assert_eq!(pick_event(&evs, NOW, Some("meet"), None).unwrap().id, "m");
    // No platform hint: the nearest start.
    assert_eq!(pick_event(&evs, NOW, None, None).unwrap().id, "m");
    // Only the chosen calendars count.
    assert_eq!(
        pick_event(&evs, NOW, None, Some(&["h".to_string()])).unwrap().id,
        "soon"
    );
    assert!(pick_event(&evs, NOW, None, Some(&[])).is_none());
}

#[test]
fn pick_event_skips_all_day_ended_and_far_future() {
    let mut all_day = ev("d", "w", NOW - H, NOW + 23 * H);
    all_day.all_day = true;
    let evs = vec![
        all_day,
        ev("ended", "w", NOW - 2 * H, NOW),
        ev("later", "w", NOW + EARLY_MS + 1, NOW + H),
    ];
    assert!(pick_event(&evs, NOW, None, None).is_none());
}

#[test]
fn upcoming_and_links() {
    let db = db();
    let c = db.calendar();
    let mut all_day = ev("d", "w", NOW, NOW + 24 * H);
    all_day.all_day = true;
    c.replace(
        &cals(),
        &[
            ev("past", "w", NOW - 3 * H, NOW - 2 * H),
            all_day,
            ev("a", "w", NOW + H, NOW + 2 * H),
            ev("b", "h", NOW + 2 * H, NOW + 3 * H),
        ],
    )
    .unwrap();
    let ids = |v: Vec<CalendarEvent>| v.into_iter().map(|e| e.id).collect::<Vec<_>>();
    assert_eq!(ids(c.upcoming(NOW, 10, None).unwrap()), ["a", "b"]);
    assert_eq!(ids(c.upcoming(NOW, 1, None).unwrap()), ["a"]);
    assert_eq!(ids(c.upcoming(NOW, 10, Some(&["h".into()])).unwrap()), ["b"]);

    db.connection()
        .execute_batch("INSERT INTO meetings (id,title,platform,started_at,status,participants) VALUES ('m1','t','other',0,'done','[]')")
        .unwrap();
    assert!(c.link_meeting("m1", "a").unwrap());
    assert!(!c.link_meeting("nope", "a").unwrap());
    assert_eq!(c.meeting_event("m1").unwrap().as_deref(), Some("a"));
    // Clearing the cache keeps the note's link.
    c.clear().unwrap();
    assert!(c.events_from(0).unwrap().is_empty());
    assert_eq!(c.meeting_event("m1").unwrap().as_deref(), Some("a"));
}

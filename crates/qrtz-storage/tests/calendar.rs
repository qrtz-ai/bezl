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
        Some("https://us02web.zoom.us/j/123")
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
    assert_eq!(pick_event(&evs, NOW, Some("google-meet"), None).unwrap().id, "m");
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

#[test]
fn m2_links_lose_query_and_fragment_and_old_cache_is_scrubbed() {
    for (raw, want) in [
        (
            "https://us02web.zoom.us/j/123?pwd=s3cr3t",
            Some("https://us02web.zoom.us/j/123"),
        ),
        (
            "https://meet.google.com/abc-defg-hij?authuser=1#x",
            Some("https://meet.google.com/abc-defg-hij"),
        ),
        ("https://zoom.us/j/9#pwd=y", Some("https://zoom.us/j/9")),
        ("https://zoom.us?pwd=y", Some("https://zoom.us")),
        ("https://evil.example/?u=https://zoom.us", None),
    ] {
        assert_eq!(clean_meeting_url(Some(raw)).as_deref(), want, "{raw}");
    }
    // A cache written by the old code (query kept) is cleaned in place.
    let db = db();
    let c = db.calendar();
    c.replace(&cals(), &[ev("a", "w", NOW, NOW + H), ev("b", "w", NOW, NOW + H)])
        .unwrap();
    db.connection()
        .execute(
            "UPDATE calendar_events SET meeting_url='https://zoom.us/j/1?pwd=old#f' WHERE id='a'",
            [],
        )
        .unwrap();
    assert_eq!(c.scrub_links().unwrap(), 1);
    let got = c.events_from(NOW - H).unwrap();
    assert_eq!(got[0].meeting_url.as_deref(), Some("https://zoom.us/j/1"));
    assert_eq!(got[1].meeting_url, None);
    assert_eq!(c.scrub_links().unwrap(), 0, "idempotent");
}

#[test]
fn m2_urls_in_titles_lose_query_and_fragment_and_cached_titles_are_scrubbed() {
    assert_eq!(
        clean_title("sync https://zoom.us/j/1?pwd=abc and http://x.example/a#t ok?"),
        "sync https://zoom.us/j/1 and http://x.example/a ok?"
    );
    let db = db();
    let c = db.calendar();
    let mut a = ev("a", "w", NOW, NOW + H);
    a.title = "standup https://meet.google.com/abc?authuser=2".into();
    c.replace(&cals(), &[a, ev("b", "w", NOW, NOW + H)]).unwrap();
    assert_eq!(
        c.events_from(NOW - H).unwrap()[0].title,
        "standup https://meet.google.com/abc"
    );
    // A cache written by the old code keeps the passcode in the title: scrubbed in place.
    db.connection()
        .execute(
            "UPDATE calendar_events SET title='sync https://zoom.us/j/1?pwd=old' WHERE id='b'",
            [],
        )
        .unwrap();
    assert_eq!(c.scrub_links().unwrap(), 1);
    assert_eq!(
        c.events_from(NOW - H).unwrap()[1].title,
        "sync https://zoom.us/j/1"
    );
    assert_eq!(c.scrub_links().unwrap(), 0);
}

#[test]
fn m2_scheme_less_hosts_tel_passcodes_and_full_width_marks() {
    // Scheme-less meeting-host words are cut at ? / # too.
    assert_eq!(
        clean_title("sync zoom.us/j/1?pwd=s3cr3t now"),
        "sync zoom.us/j/1 now"
    );
    assert_eq!(
        clean_title("(us02web.zoom.us/j/9#success)"),
        "(us02web.zoom.us/j/9"
    );
    assert_eq!(
        clean_title("meet.google.com/abc-defg-hij?authuser=1"),
        "meet.google.com/abc-defg-hij"
    );
    assert_eq!(clean_title("www.zoom.us/j/1?pwd=x"), "www.zoom.us/j/1");
    // A word that isn't a meeting host keeps its ?.
    assert_eq!(
        clean_title("ready? example.com/a?b ok?"),
        "ready? example.com/a?b ok?"
    );
    // tel: dial strings keep only the number.
    assert_eq!(
        clean_title("dial tel:+15551234567,,98765432# now"),
        "dial tel:+15551234567 now"
    );
    assert_eq!(
        clean_title("TEL:+1-555-123-4567;postd=1234#"),
        "TEL:+1-555-123-4567"
    );
    assert_eq!(clean_title("tel:+15551234567"), "tel:+15551234567");
    // Full-width ？ / ＃ count like ? / #.
    assert_eq!(
        clean_title("https://zoom.us/j/1？pwd=s3cr3t"),
        "https://zoom.us/j/1"
    );
    assert_eq!(clean_title("zoom.us/j/1＃pwd"), "zoom.us/j/1");
    assert_eq!(
        clean_meeting_url(Some("https://zoom.us/j/1？pwd=s3cr3t")).as_deref(),
        Some("https://zoom.us/j/1")
    );
}

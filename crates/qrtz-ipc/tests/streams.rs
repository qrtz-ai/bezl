#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0095: high-rate events go over per-window channel streams. Same capability rule as subscriptions; a
//! window that closes (or reloads) drops its streams and their channels, with nothing left behind.
use qrtz_ipc::{is_stream, ErrorCode, Streams, Subscriptions};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A channel that counts live instances, so a leak shows up.
struct Chan(Arc<AtomicUsize>);
impl Chan {
    fn new(live: &Arc<AtomicUsize>) -> Self {
        live.fetch_add(1, Ordering::SeqCst);
        Self(live.clone())
    }
}
impl Drop for Chan {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[test]
fn the_high_rate_events_are_streams_and_state_events_are_not() {
    for e in [
        "note:session",
        "note:transcript",
        "speech:status",
        "data:backup-progress",
    ] {
        assert!(is_stream(e), "{e}");
    }
    // Low-rate state stays on the broadcast event; talk:state too (its levels are the pill's, ADR 0092).
    for e in [
        "config:changed",
        "note:changed",
        "note:pipeline",
        "helper:status",
        "talk:state",
        "permissions:changed",
    ] {
        assert!(!is_stream(e), "{e}");
    }
}

#[test]
fn a_sub_app_without_the_capability_cannot_open_or_join_a_stream() {
    let live = Arc::new(AtomicUsize::new(0));
    let mut s = Streams::default();
    // gallery holds no capabilities; talk lacks note.read; settings lacks note.read.
    for (app, ev) in [
        ("gallery", "note:session"),
        ("talk", "note:transcript"),
        ("settings", "note:session"),
        ("note", "data:backup-progress"),
    ] {
        let e = s.open("main", app, ev, Chan::new(&live)).unwrap_err();
        assert_eq!(e.code, ErrorCode::Forbidden, "{app} {ev}");
    }
    assert_eq!(s.len(), 0);
    assert_eq!(live.load(Ordering::SeqCst), 0, "refused channels are dropped");
    // Unknown and non-stream events, and malformed requests.
    assert_eq!(
        s.open("main", "note", "nope", Chan::new(&live)).unwrap_err().code,
        ErrorCode::UnknownChannel
    );
    assert_eq!(
        s.open("main", "note", "note:changed", Chan::new(&live))
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        s.open("main", "", "note:session", Chan::new(&live))
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );

    let id = s.open("main", "note", "note:session", Chan::new(&live)).unwrap();
    assert_eq!(
        s.join("main", id, "gallery").unwrap_err().code,
        ErrorCode::Forbidden
    );
    assert_eq!(s.join("main", id, "talk").unwrap_err().code, ErrorCode::Forbidden);
    // Another window can't join (or close) this window's stream.
    assert_eq!(s.join("hud", id, "shell").unwrap_err().code, ErrorCode::NotFound);
    assert!(!s.close("hud", id));
    s.join("main", id, "shell").unwrap();
    assert_eq!(s.sub_apps(id), vec!["note".to_owned(), "shell".to_owned()]);
}

#[test]
fn a_stream_event_cannot_be_subscribed_on_the_broadcast_path() {
    let mut subs = Subscriptions::default();
    assert_eq!(
        subs.subscribe("main", "note", "note:transcript")
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert!(subs.fanout("note:transcript").is_empty());
    // The capability check still comes first.
    assert_eq!(
        subs.subscribe("main", "gallery", "note:session")
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    subs.subscribe("main", "note", "note:changed").unwrap();
}

#[test]
fn closing_a_window_drops_its_streams_and_channels_with_no_leak() {
    let live = Arc::new(AtomicUsize::new(0));
    let mut s = Streams::default();
    let a = s.open("main", "note", "note:session", Chan::new(&live)).unwrap();
    s.open("main", "note", "note:transcript", Chan::new(&live))
        .unwrap();
    s.open("main", "settings", "speech:status", Chan::new(&live))
        .unwrap();
    let h = s.open("hud", "shell", "note:session", Chan::new(&live)).unwrap();
    assert_eq!((s.len(), live.load(Ordering::SeqCst)), (4, 4));
    // One per window per event: reopening replaces (drops) the old one.
    let a2 = s.open("main", "shell", "note:session", Chan::new(&live)).unwrap();
    assert_ne!(a, a2);
    assert_eq!((s.len(), live.load(Ordering::SeqCst)), (4, 4));
    assert_eq!(
        s.fanout("note:session").map(|(id, _)| id).collect::<Vec<_>>(),
        vec![h, a2]
    );

    // The main window closes or reloads.
    assert_eq!(s.remove_target("main"), 3);
    assert_eq!((s.len(), live.load(Ordering::SeqCst)), (1, 1));
    assert_eq!(s.fanout("note:transcript").count(), 0);
    // The HUD's last listener leaves; a failed send drops a stream too.
    assert!(s.close("hud", h));
    assert!(!s.close("hud", h));
    let x = s.open("hud", "shell", "note:session", Chan::new(&live)).unwrap();
    s.drop_id(x);
    assert!(s.is_empty());
    assert_eq!(live.load(Ordering::SeqCst), 0);
}

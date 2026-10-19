//! ADR 0180: qrtz's own updater, with a fake host. Fixtures (TEST ONLY keys): tests/fixtures/app-update/README.md.
use async_trait::async_trait;
use qrtz_app::self_update::{
    archive_path, parse_feed, verify_signature, Backend, Release, SelfUpdate, State, TARGET,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const HOST: &str = "https://modules.qrtz.ai";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/app-update/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}
fn text(name: &str) -> String {
    String::from_utf8(fixture(name)).unwrap().trim().to_owned()
}
fn key() -> String {
    text("test.pub")
}

fn feed(version: &str, sig: &str) -> Value {
    let a = fixture("artifact.tar.gz");
    json!({
        "version": version,
        "notes": "faster dictation",
        "pub_date": "2026-10-19T05:00:00Z",
        "platforms": {
            TARGET: {
                "url": format!("{HOST}/{}", archive_path(version)),
                "signature": sig,
                "sha256": qrtz_modules::sha256_hex(&a),
                "size": a.len(),
            }
        }
    })
}

#[derive(Default)]
struct Fake {
    feed: Mutex<Option<Value>>,
    archive: Mutex<Vec<u8>>,
    installed: Mutex<Vec<String>>,
    fail_feed: Mutex<bool>,
}

#[async_trait]
impl Backend for Fake {
    async fn fetch_feed(&self) -> Result<Option<Vec<u8>>, String> {
        if *self.fail_feed.lock().unwrap() {
            return Err("offline".into());
        }
        Ok(self.feed.lock().unwrap().as_ref().map(|v| serde_json::to_vec(v).unwrap()))
    }
    async fn fetch_archive(&self, r: &Release, progress: &(dyn Fn(u64, u64) + Send + Sync)) -> Result<Vec<u8>, String> {
        let b = self.archive.lock().unwrap().clone();
        progress(b.len() as u64 / 2, r.size);
        progress(b.len() as u64, r.size);
        Ok(b)
    }
    async fn install(&self, r: &Release, _archive: Vec<u8>) -> Result<(), String> {
        self.installed.lock().unwrap().push(r.version.clone());
        Ok(())
    }
    fn host(&self) -> String {
        HOST.into()
    }
}

fn setup(current: &str, feed: Option<Value>) -> (Arc<SelfUpdate>, Arc<Fake>) {
    let f = Arc::new(Fake::default());
    *f.feed.lock().unwrap() = feed;
    *f.archive.lock().unwrap() = fixture("artifact.tar.gz");
    (SelfUpdate::new(current, Some(key()), f.clone()), f)
}

#[test]
fn the_feed_parses_and_pins_the_archive_path() {
    let v = feed("0.3.0", &text("good-0.3.0.sig"));
    let r = parse_feed(&serde_json::to_vec(&v).unwrap(), HOST).unwrap();
    assert_eq!(r.version, "0.3.0");
    assert_eq!(r.path, "v1/app/0.3.0/qrtz_0.3.0_aarch64.app.tar.gz");
    assert_eq!(r.notes.as_deref(), Some("faster dictation"));
    let bad = |f: &dyn Fn(&mut Value)| {
        let mut v = feed("0.3.0", &text("good-0.3.0.sig"));
        f(&mut v);
        parse_feed(&serde_json::to_vec(&v).unwrap(), HOST).unwrap_err()
    };
    // Another host, another version's archive, a prerelease, no signature, no sha256, no size, no platform.
    bad(&|v| v["platforms"][TARGET]["url"] = json!("https://evil.example/v1/app/0.3.0/qrtz_0.3.0_aarch64.app.tar.gz"));
    bad(&|v| v["platforms"][TARGET]["url"] = json!(format!("{HOST}/{}", archive_path("0.2.5"))));
    bad(&|v| v["version"] = json!("0.3.0-beta.1"));
    bad(&|v| v["platforms"][TARGET]["signature"] = json!(""));
    bad(&|v| v["platforms"][TARGET]["sha256"] = json!("abc"));
    bad(&|v| v["platforms"][TARGET]["size"] = json!(0));
    bad(&|v| v["platforms"] = json!({"darwin-x86_64": {}}));
    assert!(parse_feed(b"not json", HOST).is_err());
    assert!(parse_feed(&vec![b' '; 70_000], HOST).is_err(), "too large");
}

#[test]
fn signatures_verify_against_the_app_update_key_and_its_signed_version() {
    let a = fixture("artifact.tar.gz");
    verify_signature(&a, &text("good-0.3.0.sig"), &key(), "0.3.0").unwrap();
    // Another key, other bytes, an older release's signature paired with a newer version.
    assert!(verify_signature(&a, &text("otherkey-0.3.0.sig"), &key(), "0.3.0").is_err());
    assert!(verify_signature(&a, &text("good-0.3.0.sig"), &text("other.pub"), "0.3.0").is_err());
    let mut tampered = a.clone();
    tampered[0] ^= 1;
    assert!(verify_signature(&tampered, &text("good-0.3.0.sig"), &key(), "0.3.0").is_err());
    let e = verify_signature(&a, &text("old-0.2.5.sig"), &key(), "0.3.0").unwrap_err();
    assert!(e.contains("signed for 0.2.5"), "{e}");
    assert!(verify_signature(&a, "not base64!", &key(), "0.3.0").is_err());
}

#[tokio::test]
async fn check_download_and_ready() {
    let (u, f) = setup("0.2.0", Some(feed("0.3.0", &text("good-0.3.0.sig"))));
    assert_eq!(u.state(), State::Idle);
    assert!(matches!(u.check(false).await, State::Available(ref r) if r.version == "0.3.0"));
    let s = u.status();
    assert_eq!(s["state"], "available");
    assert_eq!(s["latest"], "0.3.0");
    assert_eq!(s["current"], "0.2.0");
    assert!(matches!(u.download().await, State::Ready(_)));
    assert_eq!(*f.installed.lock().unwrap(), ["0.3.0"]);
    assert!(u.ready());
    assert_eq!(u.status()["state"], "ready");
    // Ready stays ready: a later check doesn't undo it.
    assert!(matches!(u.check(true).await, State::Ready(_)));
}

#[tokio::test]
async fn never_a_downgrade_or_a_reinstall() {
    for v in ["0.2.0", "0.1.9"] {
        let (u, f) = setup("0.2.0", Some(feed(v, &text("good-0.3.0.sig"))));
        assert_eq!(u.check(false).await, State::UpToDate, "{v}");
        assert_eq!(u.download().await, State::UpToDate);
        assert!(f.installed.lock().unwrap().is_empty());
    }
    let (u, _) = setup("0.2.0", None);
    assert_eq!(u.check(false).await, State::UpToDate, "no feed yet");
}

#[tokio::test]
async fn a_bad_signature_or_archive_is_never_installed() {
    // Signed by another key.
    let (u, f) = setup("0.2.0", Some(feed("0.3.0", &text("otherkey-0.3.0.sig"))));
    u.check(false).await;
    assert!(matches!(u.download().await, State::Failed { ref message, .. } if message.contains("signature")));
    assert!(f.installed.lock().unwrap().is_empty());
    // An older release's valid signature paired with a newer version (a tampered feed).
    let (u, f) = setup("0.2.0", Some(feed("0.3.0", &text("old-0.2.5.sig"))));
    u.check(false).await;
    assert!(matches!(u.download().await, State::Failed { ref message, .. } if message.contains("0.2.5")));
    assert!(f.installed.lock().unwrap().is_empty());
    // Bytes that aren't the feed's.
    let (u, f) = setup("0.2.0", Some(feed("0.3.0", &text("good-0.3.0.sig"))));
    f.archive.lock().unwrap().push(b'x');
    u.check(false).await;
    assert!(matches!(u.download().await, State::Failed { .. }));
    assert!(f.installed.lock().unwrap().is_empty());
    // A failed update can be retried (the row keeps its version).
    assert_eq!(u.status()["latest"], "0.3.0");
    assert_eq!(u.status()["state"], "failed");
}

#[tokio::test]
async fn without_the_key_nothing_is_asked() {
    let f = Arc::new(Fake::default());
    *f.fail_feed.lock().unwrap() = true;
    let u = SelfUpdate::new("0.2.0", None, f.clone());
    assert_eq!(u.check(false).await, State::NotSetUp);
    assert_eq!(u.status()["state"], "not-set-up");
}

#[tokio::test]
async fn a_quiet_check_that_fails_keeps_the_row_as_it_was() {
    let (u, f) = setup("0.2.0", Some(feed("0.3.0", &text("good-0.3.0.sig"))));
    u.check(true).await;
    *f.fail_feed.lock().unwrap() = true;
    assert!(matches!(u.check(true).await, State::Available(_)), "quiet: no error shown");
    assert!(matches!(u.check(false).await, State::Failed { .. }), "pressed: the error shows");
    let (u, f) = setup("0.2.0", None);
    *f.fail_feed.lock().unwrap() = true;
    assert_eq!(u.check(true).await, State::Idle);
}

#[tokio::test]
async fn the_download_channel_answers_at_once_and_the_events_follow() {
    let events = qrtz_app::Events::new();
    let mut rx = events.subscribe();
    let f = Arc::new(Fake::default());
    *f.feed.lock().unwrap() = Some(feed("0.3.0", &text("good-0.3.0.sig")));
    *f.archive.lock().unwrap() = fixture("artifact.tar.gz");
    let u = SelfUpdate::new("0.2.0", Some(key()), f.clone()).with_events(events);
    assert_eq!(u.call("app-update:check").await.unwrap()["state"], "available");
    assert_eq!(u.call("app-update:download").await.unwrap()["state"], "downloading");
    let mut seen = vec![];
    while seen.last().map(String::as_str) != Some("ready") {
        let (name, v) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("an event")
            .unwrap();
        assert_eq!(name, "app-update:state");
        seen.push(v["state"].as_str().unwrap().to_owned());
    }
    assert!(seen.contains(&"downloading".to_owned()), "{seen:?}");
    assert!(seen.contains(&"installing".to_owned()), "{seen:?}");
    assert_eq!(u.call("app-update:status").await.unwrap()["state"], "ready");
}

/// ADR 0180: the highest signed version this install accepted is kept; nothing at or below it is offered or
/// installed again, even when it is newer than the running build and validly signed (a replayed old release).
#[tokio::test]
async fn the_high_water_mark_only_goes_up() {
    let tmp = tempfile::tempdir().unwrap();
    let hw = tmp.path().join(qrtz_app::self_update::HIGH_WATER_FILE);
    let (u, f) = setup("0.2.0", Some(feed("0.3.0", &text("good-0.3.0.sig"))));
    let u = u.with_high_water(&hw);
    assert_eq!(u.floor(), "0.2.0", "no mark yet: the build is the floor");
    u.check(false).await;
    assert!(matches!(u.download().await, State::Ready(_)));
    assert_eq!(std::fs::read_to_string(&hw).unwrap().trim(), "0.3.0");
    assert_eq!(*f.installed.lock().unwrap(), vec!["0.3.0".to_string()]);

    // Say that install was rolled back by hand: the app runs 0.2.0 again, the mark stays at 0.3.0.
    for (v, sig) in [("0.3.0", "good-0.3.0.sig"), ("0.2.5", "old-0.2.5.sig")] {
        let (u, f) = setup("0.2.0", Some(feed(v, &text(sig))));
        let u = u.with_high_water(&hw);
        assert_eq!(u.floor(), "0.3.0");
        assert_eq!(u.check(false).await, State::UpToDate, "{v}");
        assert!(matches!(u.download().await, State::UpToDate), "{v}");
        assert!(f.installed.lock().unwrap().is_empty(), "{v}");
    }
    // A mark that isn't a version is ignored (the build is the floor), never trusted to lower it.
    std::fs::write(&hw, "garbage").unwrap();
    let (u, _) = setup("0.2.0", None);
    assert_eq!(u.with_high_water(&hw).floor(), "0.2.0");
    std::fs::write(&hw, "0.1.0").unwrap();
    let (u, _) = setup("0.2.0", None);
    assert_eq!(u.with_high_water(&hw).floor(), "0.2.0");
}

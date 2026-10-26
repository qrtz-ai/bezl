//! ADR 0181: qrtz's own updater, with a fake host. Fixtures (TEST ONLY keys): tests/fixtures/app-update/README.md.
use async_trait::async_trait;
use qrtz_app::self_update::{
    archive_path, open_feed, os_supports, os_too_old_message, os_version_parts, parse_feed, parse_utc,
    verify_signature, Backend, HighWaterCommit, Release, SelfUpdate, State, TARGET,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const HOST: &str = qrtz_app::self_update::ARCHIVE_BASE;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/app-update/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
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
        "expires": "2026-11-15T00:00:00Z",
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

/// A signed envelope from the fixtures (`feed-<name>.json`, made by make.sh).
fn signed(name: &str) -> Vec<u8> {
    fixture(&format!("feed-{name}.json"))
}

/// The tests' "now": the fixture feeds expire 2026-11-15.
fn today() -> u64 {
    parse_utc("2026-10-20T12:00:00Z").unwrap()
}

#[derive(Default)]
struct Fake {
    feed: Mutex<Option<Vec<u8>>>,
    archive: Mutex<Vec<u8>>,
    installed: Mutex<Vec<String>>,
    fail_feed: Mutex<bool>,
    /// The install fails after unpacking, before the bundle check passes (the mark must not move).
    fail_install: Mutex<bool>,
}

#[async_trait]
impl Backend for Fake {
    async fn fetch_feed(&self) -> Result<Option<Vec<u8>>, String> {
        if *self.fail_feed.lock().unwrap() {
            return Err("offline".into());
        }
        Ok(self.feed.lock().unwrap().clone())
    }
    async fn fetch_archive(
        &self,
        r: &Release,
        dest: &std::path::Path,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), String> {
        let b = self.archive.lock().unwrap().clone();
        assert!(!dest.exists(), "a fresh file per download");
        progress(b.len() as u64 / 2, r.size);
        std::fs::write(dest, &b).map_err(|e| e.to_string())?;
        progress(b.len() as u64, r.size);
        Ok(())
    }
    async fn install(
        &self,
        r: &Release,
        archive: &std::path::Path,
        commit: HighWaterCommit,
    ) -> Result<(), String> {
        assert_eq!(std::fs::read(archive).unwrap(), *self.archive.lock().unwrap());
        if *self.fail_install.lock().unwrap() {
            return Err("the update isn't qrtz's notarized Developer ID build".into());
        }
        assert_eq!(commit.version(), r.version);
        commit.raise()?;
        self.installed.lock().unwrap().push(r.version.clone());
        Ok(())
    }
    fn host(&self) -> String {
        HOST.into()
    }
}

fn setup(current: &str, feed: Option<&str>) -> (Arc<SelfUpdate>, Arc<Fake>) {
    let f = Arc::new(Fake::default());
    *f.feed.lock().unwrap() = feed.map(signed);
    *f.archive.lock().unwrap() = fixture("artifact.tar.gz");
    (
        SelfUpdate::new(current, Some(key()), f.clone()).with_clock(today),
        f,
    )
}

#[test]
fn the_feed_parses_and_pins_the_archive_path() {
    let v = feed("0.3.0", &text("good-0.3.0.sig"));
    let r = parse_feed(&serde_json::to_vec(&v).unwrap(), HOST).unwrap();
    assert_eq!(r.version, "0.3.0");
    assert_eq!(r.path, "app-v0.3.0/qrtz_0.3.0_aarch64.app.tar.gz");
    assert_eq!(
        format!("{HOST}/{}", r.path),
        "https://github.com/qrtz-ai/qrtz/releases/download/app-v0.3.0/qrtz_0.3.0_aarch64.app.tar.gz"
    );
    assert_eq!(r.notes.as_deref(), Some("faster dictation"));
    let bad = |f: &dyn Fn(&mut Value)| {
        let mut v = feed("0.3.0", &text("good-0.3.0.sig"));
        f(&mut v);
        parse_feed(&serde_json::to_vec(&v).unwrap(), HOST).unwrap_err()
    };
    // Another host, another version's archive, a prerelease, no signature, no sha256, no size, no platform.
    bad(&|v| {
        v["platforms"][TARGET]["url"] =
            json!("https://evil.example/v1/app/0.3.0/qrtz_0.3.0_aarch64.app.tar.gz")
    });
    // QRT-313: the old package host, or another repo's release, is no longer the archive's place.
    bad(&|v| {
        v["platforms"][TARGET]["url"] =
            json!("https://modules.qrtz.ai/v1/app/0.3.0/qrtz_0.3.0_aarch64.app.tar.gz")
    });
    bad(&|v| {
        v["platforms"][TARGET]["url"] =
            json!("https://github.com/evil/qrtz/releases/download/app-v0.3.0/qrtz_0.3.0_aarch64.app.tar.gz")
    });
    bad(&|v| v["platforms"][TARGET]["url"] = json!(format!("{HOST}/{}", archive_path("0.2.5"))));
    bad(&|v| v["version"] = json!("0.3.0-beta.1"));
    bad(&|v| v["platforms"][TARGET]["signature"] = json!(""));
    bad(&|v| v["platforms"][TARGET]["sha256"] = json!("abc"));
    bad(&|v| v["platforms"][TARGET]["size"] = json!(0));
    bad(&|v| v["platforms"] = json!({"darwin-x86_64": {}}));
    bad(&|v| v.as_object_mut().unwrap().remove("expires").map(|_| ()).unwrap());
    bad(&|v| v["expires"] = json!("next month"));
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

/// P3.7: latest.json is an envelope whose signature (the app-update key) covers the feed's exact bytes.
#[test]
fn the_feed_is_signed_by_the_app_update_key() {
    let inner = open_feed(&signed("0.3.0"), &key()).unwrap();
    let r = parse_feed(&inner, HOST).unwrap();
    assert_eq!(r.version, "0.3.0");
    assert_eq!(r.expires, parse_utc("2026-11-15T00:00:00Z").unwrap());
    // Signed by another key, or with the right key but checked against another.
    assert!(open_feed(&signed("0.3.0-otherkey-feed"), &key())
        .unwrap_err()
        .contains("signature"));
    assert!(open_feed(&signed("0.3.0"), &text("other.pub")).is_err());
    // A feed changed after signing (a later expires, say).
    let mut env: Value = serde_json::from_slice(&signed("0.3.0")).unwrap();
    let mut f: Value = serde_json::from_slice(&inner).unwrap();
    f["expires"] = json!("2027-01-01T00:00:00Z");
    use base64::Engine;
    env["feed"] = json!(base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&f).unwrap()));
    assert!(open_feed(&serde_json::to_vec(&env).unwrap(), &key()).is_err());
    // A bare, unsigned feed.
    assert!(open_feed(
        &serde_json::to_vec(&feed("0.3.0", &text("good-0.3.0.sig"))).unwrap(),
        &key()
    )
    .is_err());
}

/// P3.7: an expired feed (a replayed old one) is refused, and so is one that claims too long a life.
#[tokio::test]
async fn an_expired_feed_is_refused() {
    let (u, f) = setup("0.2.0", Some("0.3.0-expired"));
    assert!(matches!(u.check(false).await, State::Failed { ref message, .. } if message.contains("expired")));
    assert!(f.installed.lock().unwrap().is_empty());
    // The good feed (expires 2026-11-15), judged after it expired, or long before (more than 90 days ahead, QRT-314).
    for now in [
        "2026-11-15T00:00:00Z",
        "2026-08-16T23:59:59Z",
        "2026-08-01T00:00:00Z",
    ] {
        let (u, _) = setup("0.2.0", Some("0.3.0"));
        let u = u.with_clock(move || parse_utc(now).unwrap());
        assert!(matches!(u.check(false).await, State::Failed { .. }), "{now}");
    }
    // Exactly 90 days ahead, or 75 (refused by 0.2.4's 60-day cap), is accepted (QRT-314).
    for now in ["2026-08-17T00:00:00Z", "2026-09-01T00:00:00Z"] {
        let (u, _) = setup("0.2.0", Some("0.3.0"));
        let u = u.with_clock(move || parse_utc(now).unwrap());
        assert!(matches!(u.check(false).await, State::Available(_)), "{now}");
    }
    // Quiet: logged, the row stays as it was.
    let (u, _) = setup("0.2.0", Some("0.3.0-expired"));
    assert_eq!(u.check(true).await, State::Idle);
}

/// P3.7: an available release whose feed expired before "update" was pressed isn't downloaded.
#[tokio::test]
async fn a_release_from_a_feed_that_expired_since_is_not_downloaded() {
    let f = Arc::new(Fake::default());
    *f.feed.lock().unwrap() = Some(signed("0.3.0"));
    *f.archive.lock().unwrap() = fixture("artifact.tar.gz");
    let now = Arc::new(Mutex::new(today()));
    let n2 = now.clone();
    let u = SelfUpdate::new("0.2.0", Some(key()), f.clone()).with_clock(move || *n2.lock().unwrap());
    assert!(matches!(u.check(false).await, State::Available(_)));
    *now.lock().unwrap() = parse_utc("2026-11-16T00:00:00Z").unwrap();
    assert!(matches!(u.download().await, State::Failed { ref message, .. } if message.contains("expired")));
    assert!(f.installed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn check_download_and_ready() {
    let (u, f) = setup("0.2.0", Some("0.3.0"));
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
        let (u, f) = setup("0.2.0", Some(v));
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
    let (u, f) = setup("0.2.0", Some("0.3.0-otherkey-archive"));
    u.check(false).await;
    assert!(matches!(u.download().await, State::Failed { ref message, .. } if message.contains("signature")));
    assert!(f.installed.lock().unwrap().is_empty());
    // An older release's valid signature paired with a newer version (a tampered feed).
    let (u, f) = setup("0.2.0", Some("0.3.0-old-archive-sig"));
    u.check(false).await;
    assert!(matches!(u.download().await, State::Failed { ref message, .. } if message.contains("0.2.5")));
    assert!(f.installed.lock().unwrap().is_empty());
    // Bytes that aren't the feed's.
    let (u, f) = setup("0.2.0", Some("0.3.0"));
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
    let (u, f) = setup("0.2.0", Some("0.3.0"));
    u.check(true).await;
    *f.fail_feed.lock().unwrap() = true;
    assert!(
        matches!(u.check(true).await, State::Available(_)),
        "quiet: no error shown"
    );
    assert!(
        matches!(u.check(false).await, State::Failed { .. }),
        "pressed: the error shows"
    );
    let (u, f) = setup("0.2.0", None);
    *f.fail_feed.lock().unwrap() = true;
    assert_eq!(u.check(true).await, State::Idle);
}

#[tokio::test]
async fn the_download_channel_answers_at_once_and_the_events_follow() {
    let events = qrtz_app::Events::new();
    let mut rx = events.subscribe();
    let f = Arc::new(Fake::default());
    *f.feed.lock().unwrap() = Some(signed("0.3.0"));
    *f.archive.lock().unwrap() = fixture("artifact.tar.gz");
    let u = SelfUpdate::new("0.2.0", Some(key()), f.clone())
        .with_clock(today)
        .with_events(events);
    assert_eq!(u.call("app-update:check").await.unwrap()["state"], "available");
    assert_eq!(
        u.call("app-update:download").await.unwrap()["state"],
        "downloading"
    );
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

/// ADR 0181: the highest signed version this install accepted is kept; nothing at or below it is offered or
/// installed again, even when it is newer than the running build and validly signed (a replayed old release).
#[tokio::test]
async fn the_high_water_mark_only_goes_up() {
    let tmp = tempfile::tempdir().unwrap();
    let hw = tmp.path().join(qrtz_app::self_update::HIGH_WATER_FILE);
    let (u, f) = setup("0.2.0", Some("0.3.0"));
    let u = u.with_high_water(&hw);
    assert_eq!(u.floor(), "0.2.0", "no mark yet: the build is the floor");
    u.check(false).await;
    assert!(matches!(u.download().await, State::Ready(_)));
    assert_eq!(std::fs::read_to_string(&hw).unwrap().trim(), "0.3.0");
    assert_eq!(*f.installed.lock().unwrap(), vec!["0.3.0".to_string()]);

    // Say that install was rolled back by hand: the app runs 0.2.0 again, the mark stays at 0.3.0.
    for v in ["0.3.0", "0.2.5"] {
        let (u, f) = setup("0.2.0", Some(v));
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

/// U1: the mark is raised by the install, after the new bundle passed its checks and right before the swap; an
/// install that fails before that leaves the version free to retry.
#[tokio::test]
async fn a_failed_install_leaves_the_version_free_to_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let hw = tmp.path().join(qrtz_app::self_update::HIGH_WATER_FILE);
    let (u, f) = setup("0.2.0", Some("0.3.0"));
    let u = u.with_high_water(&hw).with_work_dir(tmp.path().join("work"));
    *f.fail_install.lock().unwrap() = true;
    u.check(false).await;
    assert!(matches!(u.download().await, State::Failed { .. }));
    assert!(!hw.exists(), "a failed install doesn't burn 0.3.0");
    assert_eq!(u.floor(), "0.2.0");
    // The retry goes through and raises the mark.
    *f.fail_install.lock().unwrap() = false;
    assert!(matches!(u.download().await, State::Ready(_)));
    assert_eq!(std::fs::read_to_string(&hw).unwrap().trim(), "0.3.0");
    // P3.7: the archive was streamed to a temp file under the work dir, which is gone again.
    let left: Vec<_> = std::fs::read_dir(tmp.path().join("work")).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
    // A bad download doesn't move the mark either.
    let hw2 = tmp.path().join("hw2");
    let (u, f) = setup("0.2.0", Some("0.3.0"));
    let u = u.with_high_water(&hw2);
    f.archive.lock().unwrap().push(b'x');
    u.check(false).await;
    assert!(matches!(u.download().await, State::Failed { .. }));
    assert!(!hw2.exists());
}

// Item 5 (macOS 15 floor): the signed feed's minimumSystemVersion.

#[test]
fn macos_versions_compare_numerically() {
    assert_eq!(os_version_parts("15"), Some([15, 0, 0]));
    assert_eq!(os_version_parts("15.0.1"), Some([15, 0, 1]));
    for bad in ["", "15.", "a.b", "15.0.0.1", "1500", "-1", "15 .0"] {
        assert_eq!(os_version_parts(bad), None, "{bad}");
    }
    assert!(os_supports("15.0", Some("15")));
    assert!(os_supports("15.0", Some("15.0.1")));
    assert!(os_supports("15.0", Some("26.1")));
    assert!(os_supports("15.10", Some("15.10")));
    assert!(
        !os_supports("15.10", Some("15.9")),
        "numeric, not string, compare"
    );
    assert!(!os_supports("15.0", Some("14.7.1")));
    assert!(
        os_supports("15.0", None),
        "unknown running OS: offered (the bundle's own floor still holds)"
    );
    assert!(os_too_old_message("0.5.0", "15.0", "14.7").contains("needs macOS 15.0 or later"));
}

#[test]
fn the_feed_carries_a_valid_minimum_macos() {
    let mut v = feed("0.3.0", &text("good-0.3.0.sig"));
    assert_eq!(
        parse_feed(&serde_json::to_vec(&v).unwrap(), HOST)
            .unwrap()
            .minimum_system_version,
        None
    );
    v["minimumSystemVersion"] = json!("15.0");
    let r = parse_feed(&serde_json::to_vec(&v).unwrap(), HOST).unwrap();
    assert_eq!(r.minimum_system_version.as_deref(), Some("15.0"));
    for bad in [json!("fifteen"), json!(15), json!("15.0.0.0")] {
        v["minimumSystemVersion"] = bad;
        assert!(parse_feed(&serde_json::to_vec(&v).unwrap(), HOST).is_err());
    }
    // The signed fixtures: every feed says 15.0 except the 99.0 one.
    let r = parse_feed(&open_feed(&signed("0.3.0-macos99"), &key()).unwrap(), HOST).unwrap();
    assert_eq!(r.minimum_system_version.as_deref(), Some("99.0"));
}

#[tokio::test]
async fn an_update_for_a_newer_macos_is_skipped_with_a_message() {
    for quiet in [true, false] {
        let (u, f) = setup("0.2.0", Some("0.3.0-macos99"));
        let u = u.with_os_version(|| Some("15.1".into()));
        match u.check(quiet).await {
            State::Failed { message, release } => {
                assert!(
                    message.contains("qrtz 0.3.0 needs macOS 99.0 or later"),
                    "{message}"
                );
                assert!(message.contains("macOS 15.1"), "{message}");
                assert!(release.is_none());
            }
            s => panic!("{s:?}"),
        }
        assert!(!matches!(u.download().await, State::Ready(_)));
        assert!(
            f.installed.lock().unwrap().is_empty(),
            "never downloaded or installed"
        );
    }
    // The same Mac takes the 15.0 feed; an older Mac doesn't.
    let (u, _) = setup("0.2.0", Some("0.3.0"));
    let u = u.with_os_version(|| Some("15.1".into()));
    assert!(matches!(u.check(false).await, State::Available(_)));
    let (u, f) = setup("0.2.0", Some("0.3.0"));
    let u = u.with_os_version(|| Some("14.7.1".into()));
    assert!(matches!(u.check(false).await, State::Failed { .. }));
    assert!(f.installed.lock().unwrap().is_empty());
}

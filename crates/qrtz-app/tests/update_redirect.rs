//! QRT-313: the updater's redirect policy (Bozniak's design). GitHub release downloads start on github.com and must
//! end, over HTTPS on every hop, on exactly objects.githubusercontent.com or release-assets.githubusercontent.com;
//! anything else is `update_redirect_blocked` with the refused host and scheme.
use qrtz_app::self_update::{to_json, Release, State, ARCHIVE_BASE, FEED_URL};
use qrtz_app::update_redirect::{allow, check_chain, RedirectBlocked, CODE};

const START: &str =
    "https://github.com/qrtz-ai/qrtz/releases/download/app-v0.2.5/qrtz_0.2.5_aarch64.app.tar.gz";
const OBJECTS: &str =
    "https://objects.githubusercontent.com/github-production-release-asset/1/2?X-Amz-Signature=x";
const ASSETS: &str =
    "https://release-assets.githubusercontent.com/github-production-release-asset/1/2?sp=r&sig=x";

fn blocked(chain: &[&str]) -> RedirectBlocked {
    check_chain(chain).expect_err(&format!("{chain:?} must be blocked"))
}

#[test]
fn the_allowed_hosts_pass() {
    check_chain(&[START, OBJECTS]).unwrap();
    check_chain(&[START, ASSETS]).unwrap();
    // The feed: latest → the tagged release on github.com → the asset host.
    let tagged = "https://github.com/qrtz-ai/qrtz/releases/download/app-v0.2.5/latest.json";
    check_chain(&[FEED_URL, tagged, ASSETS]).unwrap();
    // Hosts compare case-insensitively (URLs normalize them), never by substring.
    check_chain(&[START, "https://OBJECTS.githubusercontent.com/a"]).unwrap();
    assert!(allow(OBJECTS, true) && allow(ASSETS, true));
    assert!(allow(START, false));
    assert!(FEED_URL.starts_with("https://github.com/qrtz-ai/qrtz/releases/"));
    assert!(ARCHIVE_BASE.starts_with("https://github.com/qrtz-ai/qrtz/releases/"));
}

#[test]
fn plain_http_is_blocked_on_any_hop() {
    let b = blocked(&[START, "http://objects.githubusercontent.com/a"]);
    assert_eq!(
        (b.scheme.as_str(), b.host.as_str()),
        ("http", "objects.githubusercontent.com")
    );
    let b = blocked(&[START, "http://release-assets.githubusercontent.com/a"]);
    assert_eq!(b.scheme, "http");
    // An http first hop, or an http github.com hop in the middle, even when the chain ends well.
    let b = blocked(&[
        "http://github.com/qrtz-ai/qrtz/releases/latest/download/latest.json",
        ASSETS,
    ]);
    assert_eq!((b.scheme.as_str(), b.host.as_str()), ("http", "github.com"));
    let b = blocked(&[
        FEED_URL,
        "http://github.com/qrtz-ai/qrtz/releases/download/app-v0.2.5/latest.json",
        ASSETS,
    ]);
    assert_eq!((b.scheme.as_str(), b.host.as_str()), ("http", "github.com"));
    for s in ["ftp", "file", "ws", "wss", "data"] {
        assert!(
            !allow(&format!("{s}://objects.githubusercontent.com/a"), true),
            "{s}"
        );
    }
}

#[test]
fn lookalike_hosts_are_blocked() {
    for host in [
        "objects.githubusercontent.com.evil.com",
        "evilobjects.githubusercontent.com",
        "release-assets.githubusercontent.com.evil.com",
        "evilrelease-assets.githubusercontent.com",
        "x.objects.githubusercontent.com",
        "githubusercontent.com",
        "objects-githubusercontent.com",
        "objects.githubusercontent.co",
        "objects.githubusercontent.com.",
        "github.com.evil.com",
        "evilgithub.com",
    ] {
        let b = blocked(&[START, &format!("https://{host}/a")]);
        assert_eq!(b.scheme, "https", "{host}");
        assert_eq!(b.host, host, "{host}");
    }
    // User info, an explicit port, an IP address: never the asset host, whatever the text says.
    for u in [
        "https://objects.githubusercontent.com@evil.com/a",
        "https://objects.githubusercontent.com:8443/a",
        "https://objects.githubusercontent.com:443@evil.com/a",
        "https://user:pw@objects.githubusercontent.com/a",
        "https://140.82.112.3/a",
        "https://[::1]/a",
        "not a url",
        "",
    ] {
        assert!(!allow(u, true) && !allow(u, false), "{u}");
    }
    assert_eq!(
        blocked(&[START, "https://objects.githubusercontent.com@evil.com/a"]).host,
        "evil.com"
    );
}

#[test]
fn other_hosts_are_blocked() {
    for u in [
        "https://evil.example/qrtz_0.2.5_aarch64.app.tar.gz",
        "https://modules.qrtz.ai/v1/app/0.2.5/qrtz_0.2.5_aarch64.app.tar.gz",
        "https://raw.githubusercontent.com/qrtz-ai/qrtz/main/x",
        "https://codeload.github.com/qrtz-ai/qrtz/x",
    ] {
        let b = blocked(&[START, u]);
        assert_eq!(b.scheme, "https", "{u}");
    }
    // github.com may start the chain but is never where the bytes come from.
    assert!(!allow(START, true));
    assert_eq!(blocked(&[START]).host, "github.com");
    assert!(check_chain(&[]).is_err());
}

#[test]
fn a_chain_through_a_bad_hop_is_blocked_even_if_it_ends_on_an_allowed_host() {
    let b = blocked(&[START, "https://evil.example/bounce", OBJECTS]);
    assert_eq!((b.scheme.as_str(), b.host.as_str()), ("https", "evil.example"));
    let b = blocked(&[START, "https://objects.githubusercontent.com.evil.com/x", ASSETS]);
    assert_eq!(b.host, "objects.githubusercontent.com.evil.com");
    let b = blocked(&[START, OBJECTS, "http://objects.githubusercontent.com/again"]);
    assert_eq!(b.scheme, "http");
    let b = blocked(&[START, OBJECTS, "https://evil.example/last"]);
    assert_eq!(b.host, "evil.example");
}

fn release() -> Release {
    Release {
        version: "0.2.5".into(),
        notes: None,
        path: "app-v0.2.5/qrtz_0.2.5_aarch64.app.tar.gz".into(),
        signature: "sig".into(),
        sha256: "0".repeat(64),
        size: 1,
        expires: 0,
    }
}

#[test]
fn the_status_names_the_code_host_and_scheme_for_the_settings_row() {
    let b = RedirectBlocked {
        host: "evil.example".into(),
        scheme: "http".into(),
    };
    assert_eq!(b.message(), "update_redirect_blocked: http://evil.example");
    assert_eq!(RedirectBlocked::from_message(&b.message()), Some(b.clone()));
    let s = State::Failed {
        message: b.message(),
        release: Some(release()),
    };
    let v = to_json("0.2.4", &s, None);
    assert_eq!(v["state"], "failed");
    assert_eq!(v["errorCode"], CODE);
    assert_eq!(v["errorDetails"]["host"], "evil.example");
    assert_eq!(v["errorDetails"]["scheme"], "http");
    // The `app-update:state` event with the code passes the IPC contract (else the row never sees it).
    qrtz_ipc::validate_event("app-update:state", &v).unwrap();
    let bad = serde_json::json!({ "errorCode": "something_else" });
    let mut w = v.clone();
    w["errorCode"] = bad["errorCode"].clone();
    assert!(qrtz_ipc::validate_event("app-update:state", &w).is_err());
    // Any other failure carries no code.
    for m in [
        "the update's sha256 isn't the feed's",
        "update_redirect_blocked",
        "x update_redirect_blocked: https://a",
    ] {
        let v = to_json(
            "0.2.4",
            &State::Failed {
                message: m.into(),
                release: None,
            },
            None,
        );
        assert!(v["errorCode"].is_null() && v["errorDetails"].is_null(), "{m}");
    }
    assert!(to_json("0.2.4", &State::UpToDate, None)["errorCode"].is_null());
}

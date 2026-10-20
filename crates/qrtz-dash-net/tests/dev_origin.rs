//! Bozniak hardening item 1/2/5: the dev-only origin override (docs/dash-local.md). FOR BOZNIAK REVIEW.
//! Run both ways: `cargo test -p qrtz-dash-net` (no override compiled) and
//! `cargo test -p qrtz-dash-net --features dev-local` (debug: override compiled), plus `--release --features dev-local`.
use qrtz_dash_net::*;

#[test]
fn dev_origin_accepts_only_http_127_0_0_1_with_a_port() {
    for ok in [
        "http://127.0.0.1:8799",
        "http://127.0.0.1:1",
        "http://127.0.0.1:65535",
    ] {
        assert!(dev_origin_ok(ok), "{ok}");
    }
    for bad in [
        "http://127.0.0.1",
        "http://127.0.0.1:",
        "http://127.0.0.1:0",
        "http://127.0.0.1:65536",
        "http://127.0.0.1:08799",
        "http://127.0.0.1:8799/",
        "http://127.0.0.1:8799/v1",
        "http://127.0.0.1:8799?x",
        "http://127.0.0.1:8799#x",
        "http://127.0.0.1:8799@evil.example",
        "http://user@127.0.0.1:8799",
        "http://localhost:8799",
        "http://[::1]:8799",
        "http://127.0.0.2:8799",
        "http://127.0.0.1.evil.example:8799",
        "https://127.0.0.1:8799",
        "https://dash.qrtz.ai",
        "http://dash.qrtz.ai:80",
        " http://127.0.0.1:8799",
        "http://127.0.0.1:8799 ",
    ] {
        assert!(!dev_origin_ok(bad), "{bad}");
    }
}

/// Without the override compiled in (release, or no `dev-local`), the variable does nothing.
#[cfg(not(all(debug_assertions, feature = "dev-local")))]
#[test]
fn no_override_path_without_dev_local_debug() {
    assert!(!std::hint::black_box(DEV_OVERRIDE_COMPILED));
    // SAFETY: only this test touches the variable, and this build never reads it.
    unsafe { std::env::set_var(DEV_ORIGIN_VAR, "http://127.0.0.1:8799") };
    assert_eq!(origin(), ORIGIN);
    assert!(!dev_origin_active());
    assert_eq!(keychain_service(), KEYCHAIN_SERVICE);
    assert!(origin_config_ok());
    assert!(!egress_ok("http://127.0.0.1:8799/v1/owner/view"));
    assert!(egress_ok("https://dash.qrtz.ai/v1/owner/view"));
}

#[cfg(all(debug_assertions, feature = "dev-local"))]
#[test]
fn override_on_reaches_only_the_loopback_origin_with_the_dev_keychain() {
    assert!(std::hint::black_box(DEV_OVERRIDE_COMPILED));
    // SAFETY: this test binary sets the variable before any thread reads it, and only this test reads it.
    unsafe { std::env::set_var(DEV_ORIGIN_VAR, "http://127.0.0.1:8799") };
    assert_eq!(origin(), "http://127.0.0.1:8799");
    assert!(dev_origin_active());
    assert_eq!(keychain_service(), "qrtz-dash-dev");
    assert!(egress_ok("http://127.0.0.1:8799/v1/owner/view"));
    for bad in [
        "https://dash.qrtz.ai/v1/owner/view",
        "http://127.0.0.1:87990/v1/owner/view",
        "http://127.0.0.1:8799@evil.example/v1",
        "http://127.0.0.1:8799/../x",
        "http://127.0.0.1:8799/v1#x",
        "http://evil.example/v1",
    ] {
        assert!(!egress_ok(bad), "{bad}");
    }
}

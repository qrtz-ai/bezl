#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Meeting playback: the renderer's `<audio>` loads `qrtz-media://rec/<uuid>.wav` (MeetingDetail.tsx, served by
//! apps/desktop/src-tauri/src/media.rs). Two CSPs apply to that page: the `<meta>` in apps/desktop/index.html (the
//! only one under `tauri dev`, and still enforced in builds, where Tauri adds tauri.conf.json's alongside it; a
//! browser enforces every policy). If either one's media-src leaves out `qrtz-media:`, WebKit refuses the load and
//! the player shows "Error" where the time goes (Jared's Mac, v0.2.0: the meta still allowed only `asset:`).
use serde_json::Value;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn directive(csp: &str, name: &str) -> Vec<String> {
    csp.split(';')
        .map(str::trim)
        .find_map(|d| d.strip_prefix(name).filter(|r| r.starts_with(' ')))
        .unwrap_or_else(|| panic!("CSP has no {name}: {csp}"))
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn meta_csp() -> String {
    let html = std::fs::read_to_string(root().join("apps/desktop/index.html")).unwrap();
    let at = html
        .find("http-equiv=\"Content-Security-Policy\"")
        .expect("index.html has a CSP meta");
    let rest = &html[at..];
    let start = rest.find("content=\"").expect("CSP meta content") + "content=\"".len();
    rest[start..][..rest[start..].find('"').unwrap()].to_owned()
}

fn conf_csp() -> String {
    let conf: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("apps/desktop/src-tauri/tauri.conf.json")).unwrap(),
    )
    .unwrap();
    conf["app"]["security"]["csp"].as_str().unwrap().to_owned()
}

#[test]
fn the_renderer_plays_recordings_from_qrtz_media() {
    let detail =
        std::fs::read_to_string(root().join("packages/features/note/src/MeetingDetail.tsx")).unwrap();
    assert!(
        detail.contains("qrtz-media://rec/"),
        "MeetingDetail no longer plays qrtz-media://rec/: update this test and both CSPs"
    );
}

#[test]
fn both_csps_let_the_audio_player_load_qrtz_media() {
    for (which, csp) in [("index.html meta", meta_csp()), ("tauri.conf.json", conf_csp())] {
        let media = directive(&csp, "media-src");
        for want in ["qrtz-media:", "http://qrtz-media.localhost"] {
            assert!(
                media.iter().any(|s| s == want),
                "{which} media-src {media:?} has no {want}: recordings show \"Error\" in the player"
            );
        }
        // Same rule as offline_graph.rs: nothing remote.
        for src in &media {
            assert!(
                !src.starts_with("http") || src.ends_with(".localhost"),
                "{which} media-src allows {src}"
            );
            assert!(
                src != "*" && src != "https:" && src != "http:",
                "{which} media-src allows {src}"
            );
        }
    }
}

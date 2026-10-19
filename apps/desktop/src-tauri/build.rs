fn main() {
    stub_qrtz_command_for_debug_builds();
    // App commands get generated `allow-<command>` permissions that capabilities/default.json grants.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "qrtz_invoke",
            "qrtz_subscribe",
            "qrtz_stream_open",
            "qrtz_stream_join",
            "qrtz_stream_close",
        ]),
    ))
    .expect("tauri build script");
}

/// ADR 0160, ADR 0159: on macOS `binaries/qrtz` (the `qrtz` command) and `binaries/qrtz-snap` (snap's helper) are
/// externalBins, which tauri-build requires to exist. `bun run build:helper` builds the real ones. A debug build
/// (`cargo check`, `tauri dev`, the CI check) without them gets a stub that says how to build them; a release build
/// never does, so a bundle can't ship a stub. (`binaries/qrtz-helper` has always been required as is.)
const STUB_MARK: &str = "qrtz: dev stub";
const STUBBED: &[(&str, &str)] = &[
    (
        "qrtz",
        "cargo run -p qrtz-app --features runtime --bin qrtz -- \"$@\"",
    ),
    ("qrtz-snap", "bun run build:helper"),
];

fn stub_qrtz_command_for_debug_builds() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let is_stub = |b: &[u8]| b.windows(STUB_MARK.len()).any(|w| w == STUB_MARK.as_bytes());
    for (name, how) in STUBBED {
        let bin = std::path::Path::new("binaries").join(format!("{name}-{target}"));
        println!("cargo:rerun-if-changed={}", bin.display());
        match std::fs::read(&bin) {
            Ok(b) if release && is_stub(&b) => panic!(
                "{} is the dev stub: run `bun run build:helper` (it builds {name}) before a release build",
                bin.display()
            ),
            Ok(_) => continue,
            Err(_) if release => panic!(
                "{} is missing: run `bun run build:helper` (it builds {name}) before a release build",
                bin.display()
            ),
            Err(_) => {}
        }
        std::fs::create_dir_all("binaries").expect("binaries dir");
        std::fs::write(
            &bin,
            format!("#!/bin/sh\necho '{STUB_MARK} ({name}); run {how}' >&2\nexit 1\n"),
        )
        .expect("stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
}

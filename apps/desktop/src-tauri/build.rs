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

/// ADR 0162, ADR 0174, ADR 0237: on macOS `binaries/qrtz` (the `qrtz` command) and `binaries/qrtz-<id>` (echo, nano,
/// ctrl and snap, the module helpers qrtz.app bundles) are the externalBins, which tauri-build requires to exist.
/// `bun run build:helper` builds the real ones. A debug build
/// (`cargo check`, `tauri dev`, the CI check) without it gets a stub that says how to build it; a release build
/// never does, so a bundle can't ship a stub.
/// tauri-build copies these into target/<profile>/ only when cargo reruns this script, which is mtime-based, so
/// scripts/sidecars.mjs gives a changed binary a fresh mtime and syncs target/debug itself; qrtz-helper-bridge's
/// `is_dev_stub` refuses to spawn a stub that still got through (its marker is STUB_MARK).
const STUB_MARK: &str = "qrtz: dev stub";
const STUBBED: &[(&str, &str)] = &[
    (
        "qrtz",
        "cargo run -p qrtz-app --features runtime --bin qrtz -- \"$@\"",
    ),
    ("qrtz-echo", "bun run build:helper"),
    ("qrtz-nano", "bun run build:helper"),
    ("qrtz-ctrl", "bun run build:helper"),
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

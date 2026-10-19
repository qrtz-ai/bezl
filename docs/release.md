# Release (macOS)

The goal is a signed, notarized qrtz that opens without Gatekeeper warnings. The app is Tauri 2 (ADR 0061); the
bundler signs, then `scripts/release/sign-mac.sh` re-signs the nested executables (the bundler gives every one the
app's entitlements; ADR 0160), reseals, notarizes and staples.

CI builds an unsigned Apple Silicon release from a `v*` tag (`.github/workflows/release.yml`, docs/ci.md); the
steps below are the local, signed path.

Step by step, with the exact commands and checks: [docs/modules/production-setup.md](modules/production-setup.md) Part A.

## One-time setup

1. **Apple Developer Program** membership on Jared's Apple ID.
2. Create a **Developer ID Application** certificate (Xcode → Settings → Accounts) and check it with
   `security find-identity -v -p codesigning`.
3. Notarization credentials: a notarytool keychain profile `qrtz-notary` (`xcrun notarytool store-credentials`,
   production-setup.md A3). Keep `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_API_KEY` unset for `bun run make`, so the bundler
   doesn't notarize before the nested signatures are fixed.

## Cut a release

```sh
bun install --frozen-lockfile && bun run check && bun run check:rust
bun run build:helper                               # src-tauri/binaries/{qrtz-helper,qrtz-snap,qrtz}-<target>
export APPLE_SIGNING_IDENTITY="Developer ID Application: Jared Grimes (TEAMID1234)"
export QRTZ_SPEECH_URL=… QRTZ_SPEECH_KEY_ID=… QRTZ_SPEECH_KEY=… QRTZ_SPEECH_ROOT_KEY_ID=… QRTZ_SPEECH_ROOT_KEY=…
bun run make                                       # tauri build → signed .app under the cargo target dir
scripts/release/sign-mac.sh target/release/bundle/macos/qrtz.app   # nested signatures, notarize, staple, .dmg
```

The speech module source (download URL and the two pinned Ed25519 keys, hex) is baked in at compile time
(`SpeechSetup::from_vars`, ADR 0046 §9.4). Without them the build has no speech module.

## Checklist

| Item                                                                                                 | Where                                                        | Status                     |
| ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ | -------------------------- |
| Developer ID signing of the app and `qrtz-helper` (external bin)                                     | Tauri bundler (`APPLE_SIGNING_IDENTITY`)                     | ☐ needs certificate        |
| `qrtz` (`ai.qrtz.cli`) and `qrtz-snap` (`ai.qrtz.snap`) signed with no entitlements                  | `scripts/release/sign-mac.sh` (confinement-map bundledCode)  | ☑ script; ☐ first run      |
| Hardened runtime + entitlements (`device.audio-input`)                                               | `apps/desktop/src-tauri/entitlements.mac.plist`              | ☑                          |
| Usage strings: `NSMicrophoneUsageDescription`, `NSAudioCaptureUsageDescription`                      | `apps/desktop/src-tauri/Info.plist`                          | ☑                          |
| Notarization + stapling                                                                              | `scripts/release/sign-mac.sh` (`qrtz-notary` profile)        | ☐ needs credentials        |
| Dev-only knobs ignored in release builds (N87)                                                       | `DevKnobs::from_env` (`QRTZ_ALLOW_DEV_KNOBS=1` for E2E only) | ☑                          |
| Speech module source baked in                                                                        | `QRTZ_SPEECH_*` at build time                                | ☐ needs the release server |
| Release builds never read `QRTZ_SPEECH_*` at run time, even with `QRTZ_ALLOW_DEV_KNOBS=1` (ADR 0078) | `backend.rs` `speech_var` + `speech_source_tests`            | ☑                          |
| System-audio and mic capture on a real Mac                                                           | docs/mac-verify.md                                           | ☐ verify on hardware       |
| App auto-update                                                                                      | not wired (Tauri updater plugin is the follow-up)            | ☐                          |
| Version bump + changelog                                                                             | `apps/desktop/src-tauri/Cargo.toml`, `tauri.conf.json`       | ☐ per release              |

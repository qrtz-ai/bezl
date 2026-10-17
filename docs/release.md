# Release (macOS)

The goal is a signed, notarized qrtz that opens without Gatekeeper warnings. The app is Tauri 2 (ADR 0061); the
bundler signs and notarizes when Apple credentials are in the environment.

## One-time setup

1. **Apple Developer Program** membership on Jared's Apple ID.
2. Create a **Developer ID Application** certificate (Xcode → Settings → Accounts) and check it with
   `security find-identity -v -p codesigning`.
3. Notarization credentials: an App Store Connect API key (`APPLE_API_ISSUER`, `APPLE_API_KEY`,
   `APPLE_API_KEY_PATH`) or `APPLE_ID` + `APPLE_PASSWORD` (app-specific) + `APPLE_TEAM_ID`.

## Cut a release

```sh
npm ci && npm run check && npm run check:rust
npm run build:helper                               # Swift helper → src-tauri/binaries/qrtz-helper-<target>
export APPLE_SIGNING_IDENTITY="Developer ID Application: Jared Grimes (TEAMID1234)"
export QRTZ_SPEECH_URL=… QRTZ_SPEECH_KEY_ID=… QRTZ_SPEECH_KEY=… QRTZ_SPEECH_ROOT_KEY_ID=… QRTZ_SPEECH_ROOT_KEY=…
npm run make                                       # tauri build → .app + .dmg under the cargo target dir
```

The speech module source (download URL and the two pinned Ed25519 keys, hex) is baked in at compile time
(`SpeechSetup::from_vars`, ADR 0046 §9.4). Without them the build has no speech module.

## Checklist

| Item                                                                            | Where                                                        | Status                     |
| ------------------------------------------------------------------------------- | ------------------------------------------------------------ | -------------------------- |
| Developer ID signing of the app and `qrtz-helper` (external bin)                | Tauri bundler (`APPLE_SIGNING_IDENTITY`)                     | ☐ needs certificate        |
| Hardened runtime + entitlements (`device.audio-input`)                          | `apps/desktop/src-tauri/entitlements.mac.plist`              | ☑                          |
| Usage strings: `NSMicrophoneUsageDescription`, `NSAudioCaptureUsageDescription` | `apps/desktop/src-tauri/Info.plist`                          | ☑                          |
| Notarization + stapling                                                         | Tauri bundler (`APPLE_*` env)                                | ☐ needs credentials        |
| Dev-only knobs ignored in release builds (N87)                                  | `DevKnobs::from_env` (`QRTZ_ALLOW_DEV_KNOBS=1` for E2E only) | ☑                          |
| Speech module source baked in                                                   | `QRTZ_SPEECH_*` at build time                                | ☐ needs the release server |
| System-audio and mic capture on a real Mac                                      | docs/mac-verify.md                                           | ☐ verify on hardware       |
| App auto-update                                                                 | not wired (Tauri updater plugin is the follow-up)            | ☐                          |
| Version bump + changelog                                                        | `apps/desktop/src-tauri/Cargo.toml`, `tauri.conf.json`       | ☐ per release              |

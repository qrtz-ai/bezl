# Release readiness (macOS)

The goal is a signed, notarized qrtz that opens without Gatekeeper warnings and can update itself.
Everything below is wired in. Only the Apple credentials are missing.

## One-time setup

1. **Apple Developer Program** membership ($99/yr) on Jared's Apple ID.
2. In Xcode → Settings → Accounts, create a **Developer ID Application** certificate. Check that it
   shows up with `security find-identity -v -p codesigning`.
3. Store notarization credentials in the keychain. This is preferred over env passwords:
   ```sh
   xcrun notarytool store-credentials qrtz-notary \
     --apple-id you@example.com --team-id TEAMID1234 --password <app-specific-password>
   ```
   Create the app-specific password at appleid.apple.com → Sign-In and Security.

## Cut a release

```sh
export QRTZ_MAC_SIGN_IDENTITY="Developer ID Application: Jared Grimes (TEAMID1234)"
export QRTZ_NOTARY_PROFILE=qrtz-notary      # or APPLE_ID + APPLE_APP_SPECIFIC_PASSWORD + APPLE_TEAM_ID
QRTZ_ARCH=arm64 scripts/release-mac.sh      # arm64 | x64 | both
```

The script runs `npm ci` and `npm run check`. It then builds and tests the Swift helper and runs
`electron-forge make`. Signing and notarization happen inside that step through `osxSign` and
`osxNotarize` in `apps/desktop/forge.config.mts`. Both are active only when the env vars above are
set, so unsigned local builds work as before. The script finishes by verifying the result and
stapling the DMG.

## Checklist

| Item                                                                                                                       | Where                                                               | Status                                                                    |
| -------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| Developer ID signing of the app, Electron helpers, frameworks, `qrtz-helper` and the unpacked onnxruntime `.node`/`.dylib` | `osxSign` (@electron/osx-sign signs nested code)                    | ☑ wired, needs certificate                                                |
| Hardened runtime on every signed file                                                                                      | `optionsForFile → hardenedRuntime: true`                            | ☑                                                                         |
| Entitlements: `cs.allow-jit` (V8), `device.audio-input` (mic)                                                              | `apps/desktop/entitlements.mac.plist`                               | ☑                                                                         |
| Usage strings: `NSMicrophoneUsageDescription`, `NSAudioCaptureUsageDescription`                                            | `extendInfo`                                                        | ☑                                                                         |
| System-audio capture (Core Audio taps / ScreenCaptureKit) permission prompt on a real Mac                                  | docs/mac-verify.md                                                  | ☐ verify on hardware                                                      |
| Electron fuses: RunAsNode off, NODE_OPTIONS and inspect off, asar integrity, OnlyLoadAppFromAsar                           | `FusesPlugin`                                                       | ☑ (applied before signing)                                                |
| Notarization (app via `osxNotarize`, DMG via `notarytool submit`) and stapling                                             | forge config + script                                               | ☑ wired, needs credentials                                                |
| Gatekeeper check: `spctl --assess`, `stapler validate`, `codesign --verify --deep --strict`                                | script                                                              | ☑                                                                         |
| Dev-only env hooks stripped in packaged builds (`QRTZ_LLM_BASE_URL`, seeds, mocks)                                         | `src/main/index.ts`                                                 | ☑                                                                         |
| On-device embeddings bundled (transformers.js + onnxruntime-node, trimmed per platform)                                    | `packageAfterCopy` hook; `npm run smoke:packaged`                   | ☑ arm64 / linux-x64. ☐ darwin-x64 (no upstream binary, falls back to FTS) |
| Auto-update                                                                                                                | `src/main/updates.ts` (update-electron-app → update.electronjs.org) | ☐ see below                                                               |
| Version bump + changelog                                                                                                   | `apps/desktop/package.json` `version`                               | ☐ per release                                                             |

## Auto-update

- `update-electron-app` uses **update.electronjs.org**, which only serves **public** GitHub repos.
  It reads the release `.zip` (Squirrel.Mac requires a signed build). `qrtz-ai/qrtz` is private, so
  pick one of these options:
  1. Publish releases to a separate public repo (for example `qrtz-ai/qrtz-releases`) and change
     `repo` in `updates.ts`.
  2. Self-host a static feed: switch to `UpdateSourceType.StaticStorage` with an S3/R2 base URL.
- Updates are gated by `QRTZ_AUTO_UPDATE=1` at runtime. Before the first public release, flip the
  default in `shouldEnableUpdates` so signed packaged darwin builds are enabled unless
  `QRTZ_AUTO_UPDATE=0`.
- Upload both the `.zip` (the updater uses it) and the `.dmg` (first install) from
  `apps/desktop/out/make/`.

## Intel Macs

onnxruntime-node 1.30 ships no `darwin/x64` binary. An x64 build works, but the embedder fails to
load and chat retrieval stays lexical (FTS5). This is the existing `embedderBroken` fallback.
Options: ship arm64-only (recommended), or later add an onnxruntime-web (WASM) backend for x64.

## Verify the packaged runtime (any OS)

```sh
npm run package && xvfb-run -a npm run smoke:packaged   # drop xvfb-run on macOS
```

This launches the packaged binary with `QRTZ_EMBED_SELFTEST=1` and checks the worker log for a
384-d embedding.

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

| Item                                                                                                                       | Where                                                               | Status                                                                        |
| -------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| Developer ID signing of the app, Electron helpers, frameworks, `qrtz-helper` and the unpacked onnxruntime `.node`/`.dylib` | `osxSign` (@electron/osx-sign signs nested code)                    | ☑ wired, needs certificate                                                    |
| Hardened runtime on every signed file                                                                                      | `optionsForFile → hardenedRuntime: true`                            | ☑                                                                             |
| Entitlements: `cs.allow-jit` (V8), `device.audio-input` (mic)                                                              | `apps/desktop/entitlements.mac.plist`                               | ☑                                                                             |
| Usage strings: `NSMicrophoneUsageDescription`, `NSAudioCaptureUsageDescription`                                            | `extendInfo`                                                        | ☑                                                                             |
| System-audio capture (Core Audio taps / ScreenCaptureKit) permission prompt on a real Mac                                  | docs/mac-verify.md                                                  | ☐ verify on hardware                                                          |
| Electron fuses: RunAsNode off, NODE_OPTIONS and inspect off, asar integrity, OnlyLoadAppFromAsar                           | `FusesPlugin`                                                       | ☑ (applied before signing)                                                    |
| Notarization (app via `osxNotarize`, DMG via `notarytool submit`) and stapling                                             | forge config + script                                               | ☑ wired, needs credentials                                                    |
| Gatekeeper check: `spctl --assess`, `stapler validate`, `codesign --verify --deep --strict`                                | script                                                              | ☑                                                                             |
| Dev-only env hooks stripped in packaged builds (`QRTZ_LLM_BASE_URL`, seeds, mocks)                                         | `src/main/index.ts`                                                 | ☑                                                                             |
| On-device embeddings bundled (transformers.js + onnxruntime-node, trimmed per platform)                                    | `packageAfterCopy` hook; `npm run smoke:packaged`                   | ☑ native (arm64, linux-x64) and WASM fallback (darwin-x64); smoke checks both |
| Auto-update                                                                                                                | `src/main/updates.ts` (update-electron-app → update.electronjs.org) | ☐ see below                                                                   |
| Version bump + changelog                                                                                                   | `apps/desktop/package.json` `version`                               | ☐ per release                                                                 |

## Auto-update

**Decision:** use a self-hosted static feed and leave updates off by default. Don't make the source
repo public, and don't create accounts until we ship.

- **How it works:** if `QRTZ_UPDATE_FEED_URL` is set when `scripts/release-mac.sh` runs, the URL is
  baked into the build through a Vite `define`, and auto-update turns on (`QRTZ_AUTO_UPDATE=1`).
  `src/main/updates.ts` then uses update-electron-app's `StaticStorage` source. Squirrel.Mac reads
  `<feed>/RELEASES.json`, then downloads the zip it points to. It only installs updates signed by
  the same Developer ID.
- **Generated files:** with the feed set, the zip maker writes `out/make/zip/darwin/<arch>/RELEASES.json`.
  It merges the new release with the feed's live manifest. Upload that file and the zip to the feed
  location, and the DMG for first installs.
- **Supported URLs** (HTTPS only; anything else disables updates):
  - R2/S3/any static host: `https://updates.example.com/qrtz/{platform}/{arch}`. The placeholders
    are filled at runtime and at make time, so arm64 and x64 get separate manifests.
  - GitHub Releases on a public releases-only repo: `https://github.com/<org>/<repo>/releases/latest/download`.
    Upload `RELEASES.json` and the zip as release assets. This URL has no per-arch path, so ship
    one arch there.
- **Without a feed:** `QRTZ_AUTO_UPDATE=1` at runtime falls back to update.electronjs.org for
  `qrtz-ai/qrtz`. That service only works if the repo is public, so don't rely on it.
- **Security:** packaged builds read the feed only from the build, never from runtime env, so a
  launched process can't be pointed at a different feed. Squirrel.Mac also checks the code
  signature of every update.
- **When we ship:** create the bucket or releases repo, pick its URL, and set `QRTZ_UPDATE_FEED_URL`
  for the release build.

## Intel Macs

onnxruntime-node 1.30 ships no `darwin/x64` binary. On Intel Macs the worker detects that and uses
onnxruntime-web (single-threaded WASM, bundled at about 14 MB). It produces the same vectors, so
the index is shared. You can force either backend with `QRTZ_EMBEDDINGS_BACKEND=native|wasm`.
On Linux, the WASM path embeds in ~0.4 s cold, versus ~0.2 s for native.

## Verify the packaged runtime (any OS)

```sh
npm run package && xvfb-run -a npm run smoke:packaged   # drop xvfb-run on macOS
```

This launches the packaged binary with `QRTZ_EMBED_SELFTEST=1` and checks the worker log for a
384-d embedding from both the native and WASM backends.

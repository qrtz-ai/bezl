# Release (macOS)

The goal is a signed, notarized qrtz that opens without Gatekeeper warnings. The app is Tauri 2 (ADR 0061); the
bundler signs, then `scripts/release/sign-mac.sh` re-signs the nested executables (the bundler gives every one the
app's entitlements; ADR 0162), reseals, notarizes and staples.

CI builds an unsigned Apple Silicon release from a `v*` tag (`.github/workflows/release.yml`, docs/ci.md); the
steps below are the local, signed path.

Step by step, with the exact commands and checks: [docs/modules/production-setup.md](modules/production-setup.md) Part A.

## One-time setup

1. **Apple Developer Program** membership on Jared's Apple ID. Done: Team ID `MR26CCZ496`.
2. **Developer ID Application** certificate. Done: `Developer ID Application: Jared Grimes (MR26CCZ496)` (SHA-1
   `E689C10899E1DD9E9E582035C7AD3E563202D2FA`; `security find-identity -v -p codesigning` lists it). Both are public
   and are the repo's release defaults (`qrtz_modules::manifest::RELEASE_TEAM_ID`, the bundled lock's designated
   requirements, `confinement-map.json` `bundledCode.teamId`, the release scripts; `offline_graph` checks them).
3. Notarization credentials: a notarytool keychain profile `qrtz-notary`, made once with an app-specific password
   from account.apple.com (paste it only into notarytool's prompt, never into chat or a file):

   ```sh
   xcrun notarytool store-credentials qrtz-notary --apple-id <Jared's Apple ID> --team-id MR26CCZ496
   ```

   Keep `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_API_KEY` unset for `bun run make`, so the bundler doesn't notarize before
   the nested signatures are fixed (`release:mac` unsets them itself). The identity is deliberately not in
   `tauri.conf.json`.

## Cut a release

One command (audit P0-1), on Jared's Mac from a clean `main`:

```sh
bun install --frozen-lockfile
bun run release:mac                                  # = scripts/release/sign-notarize.sh
bun run release:mac -- --app <qrtz.app>              # an existing `bun run make` output
bun run release:mac -- --no-notarize                 # sign and check only; nothing goes to Apple
```

Defaults: `QRTZ_SIGN_ID` (else `APPLE_SIGNING_IDENTITY`) = `Developer ID Application: Jared Grimes (MR26CCZ496)`,
`QRTZ_TEAM_ID=MR26CCZ496`, `QRTZ_NOTARY_PROFILE=qrtz-notary`; export any of them to override. It prints the dmg path,
its sha256 and the Gatekeeper verdict last, and writes `<dmg>.sha256`.

The same by hand:

```sh
bun install --frozen-lockfile && bun run check && bun run check:rust
export APPLE_SIGNING_IDENTITY="Developer ID Application: Jared Grimes (MR26CCZ496)"
export QRTZ_SPEECH_URL=… QRTZ_SPEECH_KEY_ID=… QRTZ_SPEECH_KEY=… QRTZ_SPEECH_ROOT_KEY_ID=… QRTZ_SPEECH_ROOT_KEY=…
unset APPLE_ID APPLE_PASSWORD APPLE_API_KEY
bun run make                                       # build:helper + tauri build → signed .app under the cargo target dir
scripts/release/sign-mac.sh target/release/bundle/macos/qrtz.app   # nested signatures, notarize, staple, .dmg
```

It runs `bun run make` with `APPLE_SIGNING_IDENTITY=$QRTZ_SIGN_ID` and every notarization variable unset, hands the
app to `sign-mac.sh` (inside-out nested signatures from `confinement-map.json` `bundledCode`, reseal, DR, runtime and
entitlement checks, notarize and staple app and dmg), then checks that `qrtz.app` and every executable in
`Contents/MacOS` (`qrtz-app`, `qrtz`, the helpers) carry `TeamIdentifier=$QRTZ_TEAM_ID` and the hardened runtime,
re-validates the staples and Gatekeeper, and writes `<dmg>.sha256`. It refuses a missing or non-Developer-ID
identity, a team id that doesn't match it, and the placeholder team before running anything. `QRTZ_SIGN_DRY_RUN=1`
prints the plan instead (`scripts/release/sign-notarize.test.mjs` checks it on any OS).

Each notarization goes through `scripts/release/notarize.sh`: `notarytool submit --wait` can exit 0 when Apple
rejects the upload, so it asks for `--output-format json` and fails unless the status is `Accepted`. On a failure it
prints the submission id and `xcrun notarytool log <id> --keychain-profile <profile>`, which shows Apple's reasons
(`scripts/release/notarize.test.mjs`).

The speech module source (download URL and the two pinned Ed25519 keys, hex) is baked in at compile time
(`SpeechSetup::from_vars`, ADR 0046 §9.4). Without them the build has no speech module; export the `QRTZ_SPEECH_*`
variables before running the script if this release carries one.

## Updates and diagnostics

Package hosting is `modules.qrtz.ai` only (Cloudflare R2; ADR 0167 — no Backblaze B2 mirror). Publish with
`scripts/modules/publish.sh`; setup in [docs/modules/production-setup.md](modules/production-setup.md) Part B.

- **Check for updates** (settings → system) runs only when pressed: `packages:check-update` refreshes the signed
  index from the package host (verified against the pinned root) and compares its newest `qrtz` with this build. It
  installs nothing. Before the host exists (`module_hosts.json` `root1Sha256` is null) it answers "updates aren't set
  up yet" without a request.
- **Export diagnostics** (settings → system) saves one text file of the local logs plus macOS's crash, hang and spin
  reports for qrtz and its helpers (`~/Library/Logs/DiagnosticReports/qrtz*.ips`, last 30 days, at most 20, 2 MB
  each). Local only: it reads files on this Mac and writes the one file; the user sends it.

## Checklist

| Item                                                                                                 | Where                                                        | Status                     |
| ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ | -------------------------- |
| Developer ID signing of the app and its external bins (`qrtz`, `qrtz-echo`)                          | Tauri bundler (`APPLE_SIGNING_IDENTITY`)                     | ☑ certificate (MR26CCZ496) |
| `qrtz` (`ai.qrtz.cli`, no entitlements) and `qrtz-echo` (`ai.qrtz.echo`, microphone; ADR 0174)       | `scripts/release/sign-mac.sh` (confinement-map bundledCode)  | ☑ script; ☐ first run      |
| nano, ctrl, snap: signed packages, notarized at publish (ADR 0169/0170), not in qrtz.app             | `scripts/modules/build-packages.sh`, `publish.sh`            | ☑ script; ☐ first publish  |
| Hardened runtime + entitlements (`device.audio-input`)                                               | `apps/desktop/src-tauri/entitlements.mac.plist`              | ☑                          |
| Usage strings: `NSMicrophoneUsageDescription`, `NSAudioCaptureUsageDescription`                      | `apps/desktop/src-tauri/Info.plist`                          | ☑                          |
| Notarization + stapling                                                                              | `scripts/release/sign-mac.sh` (`qrtz-notary` profile)        | ☐ needs credentials        |
| Dev-only knobs ignored in release builds (N87)                                                       | `DevKnobs::from_env` (`QRTZ_ALLOW_DEV_KNOBS=1` for E2E only) | ☑                          |
| Speech module source baked in                                                                        | `QRTZ_SPEECH_*` at build time                                | ☐ needs the release server |
| Release builds never read `QRTZ_SPEECH_*` at run time, even with `QRTZ_ALLOW_DEV_KNOBS=1` (ADR 0078) | `backend.rs` `speech_var` + `speech_source_tests`            | ☑                          |
| System-audio and mic capture on a real Mac                                                           | docs/mac-verify.md                                           | ☐ verify on hardware       |
| App update check (user-started, signed index)                                                        | `packages:check-update`; installing an update is not wired   | ☑ check / ☐ install        |
| Version bump + changelog                                                                             | `apps/desktop/src-tauri/Cargo.toml`, `tauri.conf.json`       | ☐ per release              |

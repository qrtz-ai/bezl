# Release (macOS)

The goal is a signed, notarized qrtz that opens without Gatekeeper warnings. The app is Tauri 2 (ADR 0061); the
bundler signs, then `scripts/release/sign-mac.sh` re-signs the nested executables (the bundler gives every one the
app's entitlements; ADR 0162), reseals, notarizes and staples.

CI builds, signs, notarizes and (with the app-update key) publishes qrtz.app (`.github/workflows/app-release.yml`,
"qrtz.app releases" below, ADR 0181); the steps below are the local, signed path, the same scripts.

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

## Module releases in CI (ADR 0179)

nano, ctrl and snap release from GitHub Actions on GitHub-hosted macOS runners, with the release secrets in the
repo's GitHub secrets. The Mac path (`scripts/modules/publish.sh --release`) still works and is what CI runs.

- **`.github/workflows/modules-release.yml`**: on a push to main touching `modules/**`, the helper's Swift sources /
  `Package.swift` / `Package.resolved` / entitlements, `crates/qrtz-modules`, `tools/qrtz-publish`, the model pins or
  the publish scripts, plus "Run workflow" (`gh workflow run modules-release.yml [-f force_bump=true]`). One publish at a
  time (concurrency group `modules-publish`, shared with the timestamp job, never cancelled).
  1. `plan` (Linux, cheap): `scripts/ci/release-plan.sh` reads `version.mjs plan`, the `<m>-v<x.y.z>` tags and the
     live index. Nothing changed → no Mac starts. Some modules unchanged → all three are bumped (`--force-bump`),
     because `publish.sh --release` republishes every package and a rebuilt binary never matches the live one. A
     version that is live but untagged stops the run (tag it first). Missing secrets → a warning, nothing runs.
  2. `release` (macos-latest = macOS 26, arm64): Rust, bun, SwiftPM and model-hash caches;
     `scripts/ci/macos-signing.sh setup` imports the Developer ID identity into a temporary keychain, stores a
     `qrtz-notary` notarytool profile in it (Apple ID + app-specific password by default, or the App Store Connect API
     key) and writes `index.key` / `timestamp.key` 0600 into `$RUNNER_TEMP`; `scripts/ci/hydrate-out.sh` mirrors the
     bucket's `v1/` into a fresh output dir (the build continues from the live index); `publish.sh --release` bumps,
     commits and pushes `modules: release …`, builds, notarizes, signs, uploads through the R2 S3 credentials and
     tags + pushes `<m>-v<version>`; `scripts/ci/verify-live.sh` fetches every signed object back from
     modules.qrtz.ai, compares hashes and runs `qrtz-publish verify` on the fresh copy; an `always()` step deletes the
     keychain and every key file.
  - Loop guard: the bump commit is pushed with `GITHUB_TOKEN` (which never starts a workflow), and a head commit
    starting `modules: release ` or containing `[skip ci]` is skipped anyway.
- **`.github/workflows/modules-timestamp.yml`**: every day at 16:00 UTC (09:00 PDT / 08:00 PST) and on demand, on
  Linux: only the timestamp key, hydrate, `publish.sh --timestamp`, verify-live. The Mac launchd job
  (`timestamp-job.sh`) can stay as a backup; it now skips itself when the live timestamp isn't the one in its own
  `~/qrtz-publish-out` (CI published since), so a stale Mac never rolls the store back. Retire it with
  `scripts/modules/timestamp-job.sh uninstall` once CI has run green for a week.

### Secrets

Set once (and on rotation) with `scripts/ci/setup-secrets.sh` in Terminal on the release Mac. Each value goes from its
source straight into `gh secret set` on stdin; nothing is printed or written to disk.

| Secret                                                      | Source                                                                                                |
| ----------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `MACOS_CERT_P12`, `MACOS_CERT_PASSWORD`                     | the login keychain's `Developer ID Application: … (MR26CCZ496)` only, repacked with a random password |
| `AC_APPLE_ID`, `AC_APP_PASSWORD`, `AC_TEAM_ID`              | prompt (default): Apple ID + an app-specific password (account.apple.com)                             |
| `AC_API_KEY`, `AC_KEY_ID`, `AC_ISSUER_ID`                   | prompt (alternative): App Store Connect API key `.p8`, key id, issuer id                              |
| `QRTZ_INDEX_KEY`, `QRTZ_TIMESTAMP_KEY`                      | `~/.qrtz-keys/index.key`, `~/.qrtz-keys/timestamp.key` (the online keys)                              |
| `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, `R2_ACCOUNT_ID` | the keychain item `qrtz-r2` (account = access key id); account id `177665085902fbdae8b82993bb7a0b91`  |

The offline root keys (`~/qrtz-root-a`, keychain `qrtz-root-b`) are never secrets; the script refuses them. The
`qrtz-notary` profile itself can't be exported from the keychain, so its credentials are entered again. Rotation:
ADR 0179.

The workflows need a push from a token with the `workflow` scope (`gh auth refresh -s workflow`).

## qrtz.app releases (ADR 0181)

qrtz updates itself from settings → qrtz: it checks `https://modules.qrtz.ai/v1/app/latest.json` quietly at launch
(after a minute) and every 6 hours, a dot on the sidebar's qrtz row says a newer qrtz is there, "update" downloads it
with progress, and "restart to update" relaunches into it (ADR 0172's restart). Before swapping the bundle the app
checks the archive's size and sha256 (the feed's), its minisign signature against the app-update public key compiled
in (`crates/qrtz-app/src/app_update_key.pub`, its trusted comment naming the same version), that the new qrtz.app is
`ai.qrtz.desktop` at that version, signed by Developer ID team MR26CCZ496 and notarized (`codesign -R … and
notarized`, `spctl`), and that the version is above both this build and the highest one it ever accepted (the
high-water mark in its data dir). Never a downgrade, never a reinstall. `latest.json` is signed by the app-update
key and expires 30 days after it was signed; `app-feed-refresh.yml` re-signs it every Monday. If that workflow stops,
update checks fail quietly ("expired") until it runs again: run it by hand with `gh workflow run app-feed-refresh.yml`.

On R2 (bucket `qrtz-modules`, never touched by the module publish: hydrate-out.sh and publish.sh exclude `app/**`):

| Object                                          | Cache             |
| ----------------------------------------------- | ----------------- |
| `v1/app/<v>/qrtz_<v>_aarch64.app.tar.gz{,.sig}` | 1 year, immutable |
| `v1/app/<v>/qrtz_<v>_aarch64.dmg`               | 1 year, immutable |
| `v1/app/qrtz-latest.dmg` (the download link)    | 60 s              |
| `v1/app/latest.json` (uploaded last)            | 60 s              |

To cut a release: `node scripts/app/version.mjs bump` (patch: tauri.conf.json, both package.json, the workspace
Cargo.toml and Cargo.lock, CHANGELOG `## Unreleased` → `## <v> (<date>)`), commit, push to main. app-release.yml does
the rest and tags `app-v<v>`. echo is part of qrtz.app (ADR 0174): a change to echo is a qrtz release.

One-time, on the release Mac: `scripts/ci/setup-secrets.sh app-update` makes `~/.qrtz-keys/app-update.key` (0600,
no password) with `tauri signer generate`, sets the `APP_UPDATE_KEY` secret from it (stdin, never printed), writes
the public key into `crates/qrtz-app/src/app_update_key.pub` and prints only the public key. Commit that file. Until
then builds say "updates aren't set up in this build yet" and app-release.yml publishes nothing.

## Updates and diagnostics

Package hosting is `modules.qrtz.ai` only (Cloudflare R2; ADR 0167 — no Backblaze B2 mirror). Publish with
`scripts/modules/publish.sh`; setup in [docs/modules/production-setup.md](modules/production-setup.md) Part B.

- **Check for updates** (settings → qrtz) is qrtz's own updater (ADR 0181, above). `packages:check-update` (the
  signed index's newest `qrtz`) stays as a channel; settings no longer calls it.
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

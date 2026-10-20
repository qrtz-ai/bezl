# Release (macOS)

The goal is a signed, notarized qrtz that opens without Gatekeeper warnings. The app is Tauri 2 (ADR 0061); the
bundler signs, then `scripts/release/sign-mac.sh` re-signs the nested executables (the bundler gives every one the
app's entitlements; ADR 0162), reseals, notarizes and staples.

CI builds, signs, notarizes and publishes qrtz.app and every package to GitHub Releases
(`.github/workflows/release.yml`, "Releases on GitHub" below, QRT-297); the steps below are the local, signed path,
the same scripts.

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

## Releases on GitHub (QRT-297)

Every release builds, signs, notarizes and publishes in GitHub Actions (`.github/workflows/release.yml`) and is
hosted on GitHub Releases. Cloudflare R2 is no longer used. The setup steps (environments, the exact
`gh secret set --env release …` commands, notifications) are in
[docs/modules/production-setup.md](modules/production-setup.md#releases-on-github-qrt-297).

- **Tags.** `v<x.y.z>` releases qrtz.app (tauri.conf.json must say `x.y.z`); `nano-v<x.y.z>`, `ctrl-v<x.y.z>`,
  `snap-v<x.y.z>` release one package (its `modules/<m>/qrtz.json` must say `x.y.z`). Only a tag whose commit is on
  main is signed or published. "Run workflow" with `tag=<tag>` re-runs one; `tag=timestamp` and `tag=feed-refresh`
  re-sign the index timestamp and qrtz.app's update feed.
- **Jobs.** `build` compiles on macOS with no secrets and a read-only token and hands over the unsigned bundle or
  package. `sign` runs in the `release` environment (Jared approves each run), re-signs every executable inside out
  with the Developer ID (`scripts/release/sign-notarize.sh --app`, or `build-packages.sh --sign-only`), notarizes and
  staples, signs the update archive and feed (`TAURI_SIGNING_PRIVATE_KEY`) or the package index and timestamp
  (`QRTZ_INDEX_KEY`, `QRTZ_TIMESTAMP_KEY`), uploads, and checks what GitHub serves (`scripts/app/verify-live.sh`,
  `scripts/ci/verify-live.sh`). `timestamp` runs daily at 16:00 UTC with only the timestamp key, in the
  `release-timestamp` environment. Every action is pinned to a commit SHA; `scripts/release/release-workflow.test.mjs`
  checks these rules on every CI run.
- **Hosting.** The tag's own GitHub release carries the dmg or the package for people. The app reads only the fixed
  `index` release by direct download URL (`https://github.com/qrtz-ai/qrtz/releases/download/index/<asset>`, never
  `api.github.com`): layout path `v1/a/b` is the asset `v1.a.b` (`scripts/release/gh-store.sh`,
  `qrtz_modules::index::layout::on_host`). Content-addressed assets are never replaced; only `v1.timestamp.json`
  (+ `.sig`), `v1.app.latest.json` and `v1.app.qrtz-latest.dmg` are, each uploaded last.
- **Secrets** (environment `release`): `APPLE_CERT_P12_BASE64`, `APPLE_CERT_PASSWORD`, `APPLE_ID`, `APPLE_TEAM_ID`,
  `APPLE_APP_PASSWORD`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, `QRTZ_INDEX_KEY`,
  `QRTZ_TIMESTAMP_KEY`; environment `release-timestamp`: `QRTZ_TIMESTAMP_KEY` only. The offline root keys are never
  secrets. `scripts/ci/macos-signing.sh` puts the identity and notary profile in a temporary keychain and deletes it
  in an `always()` step.

## qrtz.app updates (ADR 0181)

qrtz updates itself from settings → qrtz: it checks the update feed (`v1.app.latest.json` on the `index` release)
quietly at launch (after a minute) and every 6 hours, a dot on the sidebar's qrtz row says a newer qrtz is there,
"update" downloads it with progress, and "restart to update" relaunches into it (ADR 0172's restart). Before swapping
the bundle the app checks the archive's size and sha256 (the feed's), its minisign signature against the app-update
public key compiled in (`crates/qrtz-app/src/app_update_key.pub`, its trusted comment naming the same version), that
the new qrtz.app is `ai.qrtz.desktop` at that version, signed by Developer ID team MR26CCZ496 and notarized, and that
the version is above both this build and the highest one it ever accepted. Never a downgrade, never a reinstall.
`latest.json` expires 30 days after it was signed: a release re-signs it, and so does `release.yml` with
`tag=feed-refresh` (or `scripts/app/publish-local.sh --refresh` on the Mac).

| Asset on the `index` release                    | Replaced?             |
| ----------------------------------------------- | --------------------- |
| `v1.app.<v>.qrtz_<v>_aarch64.app.tar.gz{,.sig}` | never                 |
| `v1.app.<v>.qrtz_<v>_aarch64.dmg`               | never                 |
| `v1.app.qrtz-latest.dmg` (the download link)    | each release          |
| `v1.app.latest.json` (uploaded last)            | each release, refresh |

To cut a release: `node scripts/app/version.mjs bump`, commit, push to main, then `git tag v<v> && git push origin
v<v>` and approve the run. echo is part of qrtz.app (ADR 0174): a change to echo is a qrtz release.

### Publishing from the Mac (`scripts/app/publish-local.sh`, fallback)

The Mac can still publish with the same steps the `sign` job runs, using your own `gh` login for the upload (no other
credential): `bun run release:mac`, then `scripts/app/publish-local.sh --dry-run` and `scripts/app/publish-local.sh`
(`--refresh` re-signs the live feed). It refuses a dirty tree, a HEAD that isn't on origin's main, an existing
`app-v<v>`, a live feed at or above `<v>` and anything already up for `<v>`. Tests:
`scripts/app/publish-local.test.mjs` (dry run against a throwaway clone; upload order with gh stubbed).

## Updates and diagnostics

Package hosting is the `index` GitHub release only (QRT-297; ADR 0167: no mirror). Publish with a `<m>-v<x.y.z>` tag
(release.yml) or `scripts/modules/publish.sh` on the Mac.

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

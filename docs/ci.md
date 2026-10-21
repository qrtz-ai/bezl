# CI and releases (GitHub Actions)

Both workflows live in `.github/workflows/`. They run on GitHub-hosted runners; nothing is self-hosted.

## `ci.yml`: push to main, PRs, tags, release.yml and manual runs (QRT-310)

On a push to main, a pull request or a plain manual run (`gh workflow run ci.yml --ref <branch>`), only the Linux jobs
run: `js`, `e2e`, `rust` (and `workflow-changes` on a push). The macOS jobs (`macos`, `echo-warmup`, `keychain`,
`release-unsigned`) run only for releases: a `v*` tag push, `release.yml` calling this workflow, or a manual run with
`-f macos=true` (`gh workflow run ci.yml --ref <branch> -f macos=true`).

There is one concurrency group per ref (`ci-<ref>`): a newer run on a branch, tag or PR cancels the older one, and main's
runs are never cancelled. The caches are saved only by a job that finishes.

| Job                | Runner         | Runs                                                                                                                                                                                                                     |
| ------------------ | -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `js`               | `ubuntu-24.04` | `bun install --frozen-lockfile`, `bun run lint`, `bun run typecheck`, `bun run test`                                                                                                                                     |
| `e2e`              | `ubuntu-24.04` | `bun run e2e` in apps/desktop: the renderer Playwright suite, Chromium and WebKit, mocked backend (`QRTZ_TAURI_E2E_PORT=1420`)                                                                                           |
| `rust`             | `ubuntu-24.04` | renderer build (for `frontendDist`), `cargo fmt --check`, both clippy passes of `check:rust`, `cargo test --workspace`                                                                                                   |
| `macos`            | `macos-26`     | `bun run test:helper` (Swift helper `swift build` + `swift test`, fails on 0 tests), `cargo check -p qrtz-desktop`, `cargo test --workspace`                                                                             |
| `release-unsigned` | `macos-latest` | `bun run make` in apps/desktop with no identity, ad-hoc signed like `sign-mac.sh`, unchanged `verify-bundle.sh` (only the Developer ID / notarization / staple FAILs are tolerated), `.app` + `.dmg` artifact. No caches |
| `workflow-changes` | `ubuntu-24.04` | push to main only: lists commits touching `.github/**` in the job summary (pre-public checklist)                                                                                                                         |

The repo is public: Actions minutes are free and CI also runs on `pull_request` (never `pull_request_target`). No workflow uses a secret; signing, notarizing and publishing stay on Jared's Mac.

macOS minutes count 10x on a private repo, so the macOS job only does what Linux can't: the Mac-only Swift
(AX, capture, FluidAudio, Foundation Models) and the macOS `cfg` code in the desktop crate. Its `cargo check` uses a
stub sidecar in `src-tauri/binaries/`; the real helper is built only for releases. Caches: cargo
(`Swatinem/rust-cache`), bun's install cache, SwiftPM `.build`, and the Playwright browsers.

## High-churn paths (Bozniak's personal review)

Changes to these paths need Bozniak's review of the exact diff and a macOS run on the exact head before they land
(QRT-310 rule); batch them where possible and note it on the ticket before pushing:

- `.github/workflows/*`
- `scripts/release/*`, `scripts/check-*`, `mac-gate.sh`
- entitlements (`*.entitlements`, `apps/desktop/src-tauri/entitlements.mac.plist`)
- Dash enrollment (`services/dash-worker` enroll paths)
- `native/`
- `apps/desktop/src-tauri/speech.json`: the speech module pins a release bakes in (QRT-306). Exactly `{"speech":"none"}`
  or all five `QRTZ_SPEECH_*` fields; `scripts/release/check-speech.mjs` fails the release build otherwise.

## `app-release.yml`: qrtz.app releases (ADR 0181)

Replaces `release.yml` (unsigned `v*` tag builds). It runs on a push to main whose
`apps/desktop/src-tauri/tauri.conf.json` version went up (`node scripts/app/version.mjs bump` writes every version
file and the CHANGELOG section; a commit that leaves the version alone never releases), or by hand
(`gh workflow run app-release.yml`).

1. `plan` (Linux): the version, whether it is a bump, whether `app-v<version>` exists, and which secrets are set.
2. `release` (`macos-latest`): `scripts/ci/macos-signing.sh setup-identity` (temporary keychain: the Developer ID
   identity and the notary profile, from `MACOS_CERT_*` and `AC_*`), then `scripts/release/sign-notarize.sh` (build
   with echo bundled, re-sign nested code, notarize and staple the app and the dmg, Gatekeeper checks). The step after
   checks the bundle's version and identifier and the designated requirements of qrtz.app (`and notarized`) and
   `qrtz-echo` (`ai.qrtz.echo`, MR26CCZ496). The dmg and its `.sha256` are uploaded as the artifact
   `qrtz-<version>-macos-aarch64-dmg` (30 days) on every run.
3. Only with the app-update key (`APP_UPDATE_KEY` secret + the committed `crates/qrtz-app/src/app_update_key.pub`):
   the `.app.tar.gz` is signed with `tauri signer sign --app-version`, verified with `minisign` against the committed
   public key, `scripts/app/feed.mjs` writes the feed (expires in 30 days) and `scripts/app/sign-feed.sh` signs it into `latest.json`, `scripts/app/upload.sh` uploads to R2 `v1/app/`,
   `scripts/app/verify-live.sh` compares what modules.qrtz.ai serves, and `version.mjs tag --push` tags
   `app-v<version>`. Without the key the run ends after the artifact with a warning: nothing is published.

Apple Silicon only.

The speech module source is baked in at build time from the secrets `QRTZ_SPEECH_URL`, `QRTZ_SPEECH_KEY_ID`,
`QRTZ_SPEECH_KEY`, `QRTZ_SPEECH_ROOT_KEY_ID` and `QRTZ_SPEECH_ROOT_KEY`; until they're set, release builds ship
without a speech module.

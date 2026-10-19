# CI and releases (GitHub Actions)

Both workflows live in `.github/workflows/`. They run on GitHub-hosted runners; nothing is self-hosted.

## `ci.yml`: every push to main (and manual "run workflow")

A newer push to main cancels the run still going for an older one. A manual run (`gh workflow run ci.yml`) is in its
own concurrency group, so pushes don't cancel it. The caches are saved only by a job that finishes, so when main is
busy a manual run is the way to warm them.

| Job     | Runner         | Runs                                                                                                                   |
| ------- | -------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `js`    | `ubuntu-24.04` | `bun install --frozen-lockfile`, `bun run lint`, `bun run typecheck`, `bun run test`                                   |
| `e2e`   | `ubuntu-24.04` | `bun run e2e`: the renderer Playwright suite, Chromium and WebKit, mocked backend (`QRTZ_TAURI_E2E_PORT=1420`)         |
| `rust`  | `ubuntu-24.04` | renderer build (for `frontendDist`), `cargo fmt --check`, both clippy passes of `check:rust`, `cargo test --workspace` |
| `macos` | `macos-26`     | `bun run test:helper` (Swift helper `swift build` + `swift test`, fails on 0 tests), `cargo check -p qrtz-desktop`     |

macOS minutes count 10x on a private repo, so the macOS job only does what Linux can't: the Mac-only Swift
(AX, capture, FluidAudio, Foundation Models) and the macOS `cfg` code in the desktop crate. Its `cargo check` uses a
stub sidecar in `src-tauri/binaries/`; the real helper is built only for releases. Caches: cargo
(`Swatinem/rust-cache`), bun's install cache, SwiftPM `.build`, and the Playwright browsers.

## `app-release.yml`: qrtz.app releases (ADR 0180)

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
   public key, `scripts/app/feed.mjs` writes `latest.json`, `scripts/app/upload.sh` uploads to R2 `v1/app/`,
   `scripts/app/verify-live.sh` compares what modules.qrtz.ai serves, and `version.mjs tag --push` tags
   `app-v<version>`. Without the key the run ends after the artifact with a warning: nothing is published.

Apple Silicon only.

The speech module source is baked in at build time from the secrets `QRTZ_SPEECH_URL`, `QRTZ_SPEECH_KEY_ID`,
`QRTZ_SPEECH_KEY`, `QRTZ_SPEECH_ROOT_KEY_ID` and `QRTZ_SPEECH_ROOT_KEY`; until they're set, release builds ship
without a speech module.

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

## `release.yml`: on a tag `v*`

1. Checks that the tag matches `apps/desktop/src-tauri/tauri.conf.json` `version` and takes the release notes from the
   `CHANGELOG.md` section `## X.Y.Z` (or `## vX.Y.Z`, optionally followed by a date or title) with
   `scripts/release-notes.mjs`. A missing or empty section fails the release.
2. On `macos-26`: `bun run build:helper` (release arm64 Swift helper → `src-tauri/binaries/`), then
   `tauri build --bundles app,dmg`.
3. Publishes the GitHub release with `qrtz_X.Y.Z_aarch64.dmg`, `qrtz_X.Y.Z_aarch64.app.zip` and `SHA256SUMS.txt`
   (also kept 14 days as a workflow artifact).

Apple Silicon only: a universal build would need an x86_64 helper (and FluidAudio) too. Running the workflow by hand
(`gh workflow run release.yml`) builds the same artifacts without publishing.

To cut a release: bump the version in `package.json`, `apps/desktop/package.json`, `apps/desktop/src-tauri/Cargo.toml`
and `tauri.conf.json`, rename the CHANGELOG's unreleased section to `## X.Y.Z`, push to main, then
`git tag vX.Y.Z && git push origin vX.Y.Z`.

### Not yet: signing and notarization

Builds are **unsigned** (Gatekeeper asks on first open: right-click → open). Signing and notarizing in CI need
these repository secrets, which the Tauri bundler reads from the environment of the `tauri build` step:

| Secret                       | What                                                                           |
| ---------------------------- | ------------------------------------------------------------------------------ |
| `APPLE_CERTIFICATE`          | the Developer ID Application certificate + key, exported as .p12, base64       |
| `APPLE_CERTIFICATE_PASSWORD` | the .p12 password                                                              |
| `APPLE_SIGNING_IDENTITY`     | e.g. `Developer ID Application: Jared Grimes (TEAMID1234)`                     |
| `KEYCHAIN_PASSWORD`          | any random string, for the temporary CI keychain                               |
| `APPLE_API_ISSUER`           | App Store Connect API key issuer id (notarization)                             |
| `APPLE_API_KEY`              | the API key id                                                                 |
| `APPLE_API_KEY_P8`           | the `.p8` contents; the step writes it to a file and sets `APPLE_API_KEY_PATH` |

(Instead of the API key: `APPLE_ID`, `APPLE_PASSWORD` (app-specific password) and `APPLE_TEAM_ID`.) The later step
imports the certificate into a temporary keychain before `tauri build`; the bundler then signs the app and the
`qrtz-helper` sidecar with `entitlements.mac.plist`, notarizes and staples. See docs/release.md.

The speech module source is baked in at build time from the secrets `QRTZ_SPEECH_URL`, `QRTZ_SPEECH_KEY_ID`,
`QRTZ_SPEECH_KEY`, `QRTZ_SPEECH_ROOT_KEY_ID` and `QRTZ_SPEECH_ROOT_KEY`; until they're set, release builds ship
without a speech module.

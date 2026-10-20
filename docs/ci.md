# CI and releases (GitHub Actions)

The workflows live in `.github/workflows/`. They run on GitHub-hosted runners; nothing is self-hosted.

## `ci.yml`: every push to main (and manual "run workflow")

A newer push to main cancels the run still going for an older one. A manual run (`gh workflow run ci.yml`) is in its
own concurrency group, so pushes don't cancel it. The caches are saved only by a job that finishes, so when main is
busy a manual run is the way to warm them.

| Job                | Runner         | Runs                                                                                                                                                                                                                     |
| ------------------ | -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `js`               | `ubuntu-24.04` | `bun install --frozen-lockfile`, `bun run lint`, `bun run typecheck`, `bun run test`                                                                                                                                     |
| `e2e`              | `ubuntu-24.04` | `bun run e2e` in apps/desktop: the renderer Playwright suite, Chromium and WebKit, mocked backend (`QRTZ_TAURI_E2E_PORT=1420`)                                                                                           |
| `rust`             | `ubuntu-24.04` | renderer build (for `frontendDist`), `cargo fmt --check`, both clippy passes of `check:rust`, `cargo test --workspace`                                                                                                   |
| `macos`            | `macos-26`     | `bun run test:helper` (Swift helper `swift build` + `swift test`, fails on 0 tests), `cargo check -p qrtz-desktop`, `cargo test --workspace`                                                                             |
| `release-unsigned` | `macos-latest` | `bun run make` in apps/desktop with no identity, ad-hoc signed like `sign-mac.sh`, unchanged `verify-bundle.sh` (only the Developer ID / notarization / staple FAILs are tolerated), `.app` + `.dmg` artifact. No caches |
| `workflow-changes` | `ubuntu-24.04` | push to main only: lists commits touching `.github/**` in the job summary (pre-public checklist)                                                                                                                         |

The repo is public: Actions minutes are free and CI also runs on `pull_request` (never `pull_request_target`). `ci.yml` uses no secret; only `release.yml`'s approved `sign` job and its `timestamp` job do (QRT-297).

macOS minutes count 10x on a private repo, so the macOS job only does what Linux can't: the Mac-only Swift
(AX, capture, FluidAudio, Foundation Models) and the macOS `cfg` code in the desktop crate. Its `cargo check` uses a
stub sidecar in `src-tauri/binaries/`; the real helper is built only for releases. Caches: cargo
(`Swatinem/rust-cache`), bun's install cache, SwiftPM `.build`, and the Playwright browsers.

## `release.yml`: releases on GitHub (QRT-297)

Tags `v<x.y.z>` (qrtz.app) and `nano-v*`, `ctrl-v*`, `snap-v*` (packages), "run workflow", and a daily schedule (the
index timestamp re-sign). `build` compiles with no secrets; `sign` waits for Jared's approval in the `release`
environment and runs only for a tag whose commit is on main; `timestamp` holds only the timestamp key in
`release-timestamp`. Everything is hosted on GitHub Releases (the app reads the fixed `index` release). Details:
[docs/release.md](release.md#releases-on-github-qrt-297); setup:
[docs/modules/production-setup.md](modules/production-setup.md#releases-on-github-qrt-297).

Apple Silicon only.

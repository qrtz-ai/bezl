# dev-local: run all of qrtz from this checkout

```sh
bun run dev:local                      # or scripts/dev/dev-local.sh
bun run dev:local -- --without ctrl    # leave a module out (repeatable)
bun run dev:local -- --quit-installed  # quit /Applications/qrtz.app first
bun run dev:local -- --build-only      # build and check, don't launch
```

`bun start` in apps/desktop runs this too (after building the `qrtz` command), so the checkout's modules always run in
dev. It used to run only echo from main against the real data dir, with the other modules from the installed
packages. `dev:local`:

1. `swift build`s `qrtz-nano`, `qrtz-snap`, `qrtz-ctrl` and `qrtz-echo` (native/qrtz-helper, debug) and
   `cargo build`s the app with the non-default `dev-local` feature (debug, the normal `target/`).
2. Sets `QRTZ_USER_DATA=~/Library/Application Support/ai.qrtz.desktop-dev` (override with `QRTZ_DEV_LOCAL_DATA`),
   `QRTZ_DEV_REPO` and `QRTZ_{NANO,SNAP,CTRL,ECHO}_BIN` to the `.build/debug` products.
3. Launches `tauri dev --features dev-local --config '{"identifier":"ai.qrtz.desktop.dev"}'`: the app runs as
   `ai.qrtz.desktop.dev` (not the production `ai.qrtz.desktop`) from `target/debug`.
   It logs `dev-local: <id> from <path>` per module and shows a red **dev-local** badge and window title.

## ⚠ Hazard: the dev app is a different app to macOS

It has **none of the installed qrtz's grants**. Give it its own Accessibility and Microphone grant when macOS asks.
If it runs alongside (or instead of) `/Applications/qrtz.app`, both grab fn and **echo's hotkeys break** (a debug
launch broke dictation once already). The script warns when the installed app is running and only quits it with
`--quit-installed`; it never kills it otherwise. Quit the dev app and reopen qrtz to get normal dictation back.

## Gates (src: crates/qrtz-app/src/dev_local.rs)

- Compiled in only with `debug_assertions` **and** `--features dev-local`. A release build ignores the
  environment and carries none of the override strings; `scripts/release/verify-bundle.sh` checks the binary and
  `scripts/app/publish-local.sh` refuses from a dev-local shell or when the feature is on by default.
- `QRTZ_USER_DATA` must be set, absolute, and not the real data dir or inside it. Nothing is copied from it.
- Each bin must be absolute, canonical (no symlink), a regular executable you own, not group/world-writable, and
  under `<repo>/target`, `native/qrtz-helper/.build/debug`, or `CARGO_TARGET_DIR` only when it is inside the checkout.
  The legacy `QRTZ_MODULE_HELPERS=split`, `QRTZ_<ID>_BIN` split paths and `QRTZ_HELPER_BIN` work only in a dev-local
  build too, and split-mode bins go through the same checks.
- The checkout's `modules/<id>/module.json` goes through the shared manifest checker and may not declare anything
  (capabilities, permissions, entitlements, channels, views, provides, dependencies, code identity) beyond this
  build's manifest for that id.
- An override skips only the signature and lock-hash check: the module counts as installed in the dev data dir.
  It is never switched on for you; ctrl still asks for consent and its enable prompt.
- Any failure refuses the start with a `dev-local:` error. No R2, keychain or update key is ever read.

## Privacy grants stick (one-time Accessibility grant)

dev-local signs `target/debug/qrtz-app` (`ai.qrtz.desktop.dev`) and the helpers it runs from
`native/qrtz-helper/.build/debug` (`ai.qrtz.nano`, `ai.qrtz.snap`, `ai.qrtz.ctrl`, `ai.qrtz.echo`) with your
**Apple Development** certificate (`QRTZ_DEV_SIGN_IDENTITY`, else the first one `security find-identity -v -p
codesigning` lists; a Developer ID identity is refused). The app is launched as its own responsible process
(`scripts/dev/launch.c`) and spawns the helpers, so macOS checks the **dev app's** grant, not your terminal's.

Grant Accessibility (and Microphone) to the dev app once; it survives rebuilds as long as the same certificate signs.
After switching to this signing the first time, remove the old stale rows once:
`tccutil reset Accessibility ai.qrtz.desktop.dev`, then grant again. The first signing may show a keychain prompt for
`codesign`: choose **Always Allow**. With no Apple Development identity it falls back to ad-hoc and warns; grants
then reset every build. Details: [dev-permissions.md](../dev-permissions.md).

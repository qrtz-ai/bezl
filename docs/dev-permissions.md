# macOS privacy permissions in development

qrtz has one TCC identity (ADR 0154): Accessibility, Screen Recording, Microphone and Automation are granted to the
app, and qrtz-helper, qrtz-snap and the other extension helpers use that grant because qrtz spawns them.

## Why grants "don't work" in a dev build

Two things break them for a bare `target/debug/qrtz-app`:

1. **The signature changes every build.** The linker signs it ad-hoc as `qrtz_app-<hash>`, and an ad-hoc
   designated requirement is the cdhash. Every rebuild (and `tauri dev` rebuilds constantly) is a new app to TCC,
   so the switch in System Settings stays on but `AXIsProcessTrusted()` returns false.
2. **The terminal is the responsible process.** macOS checks the grant of the process that launched the app: your
   terminal, your editor, or a remote shell. A grant given to qrtz-app itself is ignored.

## What the repo does about it

`bun start` runs `tauri dev`, which runs `cargo run` in `apps/desktop/src-tauri`. That directory's
`.cargo/config.toml` sets a macOS runner, `scripts/dev/run.sh`, which for qrtz-app (and nothing else):

- runs `scripts/dev/sign.sh`: signs `qrtz-helper`, `qrtz-snap`, `qrtz` and `qrtz-app` in `target/debug` with stable
  identifiers (`qrtz-app` is **`ai.qrtz.desktop.dev`**, the helpers use the release ids) and a real certificate;
- launches qrtz-app through `scripts/dev/launch.c`, which disclaims responsibility (as Xcode does), so qrtz-app is
  its own responsible process and its own grant is the one macOS checks.

The signing identity is `QRTZ_DEV_SIGN_ID` if set (`-` forces ad-hoc), else the first `Apple Development:` identity
in your keychain (`security find-identity -v -p codesigning`), else ad-hoc. Ad-hoc keeps the stable identifier but
TCC still pins its cdhash, so get an Apple Development certificate (Xcode → Settings → Accounts → Manage
Certificates) for grants that survive rebuilds. The first signing can show a keychain prompt for `codesign`: choose
**Always Allow**. `QRTZ_DEV_NO_SIGN=1 bun start` turns all of this off.

## Granting

Users never hunt in System Settings. Every permission prompt in qrtz is a button, "open … settings", with one line,
"switch on qrtz": qrtz-app (the responsible process) first asks macOS itself (`AXIsProcessTrustedWithOptions` with
the prompt option, `CGRequestScreenCaptureAccess`), which adds **qrtz** to that list, then opens the exact pane
(`x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility`, `…Privacy_ScreenCapture`,
`…Privacy_Microphone`, `…Privacy_ListenEvent`, `…Privacy_Automation`, `…Privacy_FilesAndFolders`). The prompt
re-reads the grant every second (`permissions:get` and `snap:status` read it live in qrtz-app; qrtz-snap re-sends
its status when its own read changes), so it disappears and snap works the moment qrtz is switched on. Screen
recording reaches a running qrtz only after a relaunch, so its row adds "restart qrtz" (`app:restart`).

- **Dev build:** start it with `bun start`. When qrtz asks, or in System Settings → Privacy & Security →
  Accessibility, switch on **qrtz-app** (click **+** and pick `target/debug/qrtz-app` if it's not listed). Grant the
  app, not your terminal. It stays granted across rebuilds as long as the same certificate signs it.
- **Installed build:** `/Applications/qrtz.app` is Developer ID signed as `ai.qrtz.desktop` and has its own row,
  **qrtz**. Open it from Finder or `open -a /Applications/qrtz.app` (not by running its binary from a terminal).
  To clear stale rows: `tccutil reset Accessibility ai.qrtz.desktop` (or `ai.qrtz.desktop.dev`).

Check what macOS sees: `codesign -dv target/debug/qrtz-app` should show `Identifier=ai.qrtz.desktop.dev` and an
`Authority=Apple Development: …` line.

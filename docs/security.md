# Security review checklist

Status as of M0 (2026-10-16). Items marked ☐ are tracked in PLAN.md.

## Renderer

- ☑ `contextIsolation: true`, `sandbox: true`, `nodeIntegration: false`, `webSecurity: true`. The e2e test asserts there's no `require` in the renderer.
- ☑ Strict CSP in `index.html` and `hud.html` (`default-src 'self'; script-src 'self'`; the same meta on both, ADR 0090; echo's pill is native, ADR 0092).
- ☑ The preload exposes only `qrtzTransport.invoke/on`. There's no raw `ipcRenderer` and no Node APIs.
- ☑ `window.open` is denied (https links go to the system browser), `will-navigate` is limited to the app origin, and `<webview>` is blocked.
- ☑ All Chromium permission requests and checks are denied. Mic, screen and AX are owned by the native helper, not the renderer.
- ☑ Renderer code (`src/shell`, features, ui) can't import `electron` or `node:*` (boundary script).

## IPC

- ☑ Every call passes through the zod envelope, then the channel and version check, then the capability check, then request validation, then the handler, then response validation.
- ☑ The sender frame URL must be the app's own origin.
- ☑ Errors are normalised to `AppError`. Internals aren't leaked, and `E_INTERNAL` is logged with its cause only in main.
- ☑ Event subscriptions are capability-checked in main. Events go only to sub apps that are allowed to receive them.
- ☐ Capability scoping is defence-in-depth among first-party, compiled-in sub apps that share one renderer. Third-party sub apps would need their own `WebContentsView` and session (future).

- ☑ Narrow channels for narrow needs: `note:set-auto-record` (note.record) instead of granting Note `config.write`.

## Secrets and config

- ☑ qrtz stores no keys (ADR 0078). There is no secret store, no `secrets:*` channel, no `secrets.write` capability and no `QRTZ_SECRET_*` env reader. At launch, Keychain items an older build saved (service `ai.qrtz.desktop`, listed in `secrets-index.json`) and any `secrets.json` are deleted; an item that can't be deleted keeps the index so the next launch retries. The logger still redacts `*key*/*token*/*secret*` fields.
- ☑ Config writes are zod-validated. Deep merge drops `__proto__`, `constructor` and `prototype` (tested).

- ☑ Dev and test env hooks (`QRTZ_DEV_SEED`, `QRTZ_LLM_BASE_URL`, `QRTZ_SIMULATE_MISSING_HELPER`, `QRTZ_MOCK_CRASH_AFTER_MS`, `QRTZ_MOCK_SCENARIO`) are stripped at startup in packaged builds, before the worker and helper inherit the env. `QRTZ_LLM_BASE_URL` could otherwise send the API key to another host.
- ☑ There is no embedding model (removed with chat, ADR 0072/0073); search is full-text only.

## Processes

- ☑ Helper and worker run under a Supervisor with backoff, a crash-loop breaker and a heartbeat watchdog.
- ☑ Helper stdout is parsed line by line with zod. Malformed lines are dropped and logged.
- ☑ Electron fuses: RunAsNode off, NODE_OPTIONS off, `--inspect` off, ASAR integrity on, OnlyLoadAppFromAsar on, and cookie encryption on.
- ☑ Crash reporting is local only (`uploadToServer: false`). The `CrashSink` hook is there for opt-in remote reporting.
- ☐ Sign and notarize with the hardened runtime. Entitlements: `com.apple.security.device.audio-input` and screen capture usage strings (M4.7/M5.3). Wired: see docs/release.md and scripts/release-mac.sh; needs a Developer ID.
- ☐ Sign the helper binary with the same Team ID. The app verifies the helper's code signature before spawning it (M4.7).

## Privacy controls (iteration 13)

- **Entirely offline** (ADR 0076): no cloud speech and no summary provider; summaries are Apple Intelligence on the Mac (ADR 0075). In qrtz's own code the only outbound requests are the signed speech-bundle downloads (qrtz-speech; plain http only to a real loopback host, N100), enforced by `qrtz-lifecycle/tests/offline_graph.rs` (ADR 0078). Open: the helper's FluidAudio model downloads (Parakeet, LocalVQE) and the OS speech-asset download still reach the network, and there is no App Sandbox (ADR 0078 follow-ups). The old `privacy.localOnly` switch is gone, and saved cloud keys are deleted at launch.
- **Logs** (ADR 0083): local only, in `~/Library/Logs/qrtz/` (folder 0700, files 0600), size-capped at 40 MiB
  (`qrtz.log` rotating at 8 MiB through `qrtz.1.log` … `qrtz.4.log`), written by a non-blocking writer that drops
  lines rather than stall the app. They never hold transcript, dictated or summary text, notes, meeting titles,
  speaker names, the dictionary, clipboard contents or secrets: call sites log ids, lengths, codes and schema paths,
  and the sink shortens the home directory to `~`, escapes control characters and cuts long lines
  (`qrtz-app/tests/logging_privacy.rs` runs a meeting and a dictation with sentinel text at `trace` and checks the
  file). A release build reads only a bare level from `QRTZ_LOG`, and third-party crates log `warn` and up. The
  renderer's `log:write` is shell-only, capped and rate-limited. Logs leave the Mac only when the user exports them
  (Settings → export logs) and sends the file.
- **Pause** (`recording.pause` / `recording.resume`): the helper writes silence and doesn't feed the recogniser; main also drops in-flight ASR.
- **Excluded apps**: detection events from listed apps or platforms are dropped in main (logged without the title).
- **Backups**: entries are restricted to `db/qrtz.db` and `audio/<safe name>.wav`. Every byte is sha256-verified before anything is written. The imported DB is integrity-checked and schema-checked, then merged without overwriting. `QRTZ_BACKUP_PATH` (which skips the dialogs, for tests) is stripped in packaged builds.

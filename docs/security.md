# Security review checklist

Status as of M0 (2026-10-16). Items marked ☐ are tracked in PLAN.md.

## Renderer

- ☑ `contextIsolation: true`, `sandbox: true`, `nodeIntegration: false`, `webSecurity: true`. The e2e test asserts there's no `require` in the renderer.
- ☑ Strict CSP in `index.html` (`default-src 'self'; script-src 'self'`).
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

- ☑ Narrow channels for narrow needs: `memo:set-auto-record` (memo.record) instead of granting Memo `config.write`.

## Secrets and config

- ☑ Secrets use `safeStorage` (Keychain-backed on macOS). The file holds only ciphertext and is mode 0600.
- ☑ On Linux, storing secrets is refused when the backend is `basic_text`.
- ☑ Secret values never cross back to the renderer. `secrets:list` returns names only, and the logger redacts `*key*/*token*/*secret*` fields.
- ☑ Config writes are zod-validated. Deep merge drops `__proto__`, `constructor` and `prototype` (tested).

- ☑ Dev and test env hooks (`QRTZ_DEV_SEED`, `QRTZ_LLM_BASE_URL`, `QRTZ_SIMULATE_MISSING_HELPER`, `QRTZ_MOCK_CRASH_AFTER_MS`, `QRTZ_MOCK_SCENARIO`) are stripped at startup in packaged builds, before the worker and helper inherit the env. `QRTZ_LLM_BASE_URL` could otherwise send the API key to another host.
- ☑ The embedding model is downloaded from Hugging Face into `userData/models`. Transcript text never leaves the device for embeddings (`QRTZ_EMBEDDINGS=0` disables them, `QRTZ_EMBEDDINGS_OFFLINE=1` forbids downloads).

## Processes

- ☑ Helper and worker run under a Supervisor with backoff, a crash-loop breaker and a heartbeat watchdog.
- ☑ Helper stdout is parsed line by line with zod. Malformed lines are dropped and logged.
- ☑ Electron fuses: RunAsNode off, NODE_OPTIONS off, `--inspect` off, ASAR integrity on, OnlyLoadAppFromAsar on, and cookie encryption on.
- ☑ Crash reporting is local only (`uploadToServer: false`). The `CrashSink` hook is there for opt-in remote reporting.
- ☐ Sign and notarize with the hardened runtime. Entitlements: `com.apple.security.device.audio-input` and screen capture usage strings (M4.7/M5.3). Wired: see docs/release.md and scripts/release-mac.sh; needs a Developer ID.
- ☐ Sign the helper binary with the same Team ID. The app verifies the helper's code signature before spawning it (M4.7).

## Privacy controls (iteration 13)

- **Local-only** (`privacy.localOnly`): `effectiveLlmProvider` maps every cloud provider to `none` before main reads any key, so summaries and chat stay on-device. Ollama on localhost stays allowed. Checked by e2e against a stub that must receive zero requests.
- **Pause** (`recording.pause` / `recording.resume`): the helper writes silence and doesn't feed the recogniser; main also drops in-flight ASR.
- **Excluded apps**: detection events from listed apps or platforms are dropped in main (logged without the title).
- **Backups**: entries are restricted to `db/qrtz.db` and `audio/<safe name>.wav`. Every byte is sha256-verified before anything is written. The imported DB is integrity-checked and schema-checked, then merged without overwriting. `QRTZ_BACKUP_PATH` (which skips the dialogs, for tests) is stripped in packaged builds.

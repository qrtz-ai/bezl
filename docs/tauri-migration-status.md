# Tauri migration status (Electron → Tauri 2 + Rust)

**Decision (Jared, 2026-10-17):** qrtz moves fully to Tauri 2 with a Rust backend. `apps/desktop-tauri` becomes
_the_ app at full feature parity with the Electron app; then `apps/desktop` (Electron) is deleted. Every hardening
learning carries over. Work lands on `main` in green increments (no branches, PRs or GitHub Actions).

If a run is interrupted, the next one continues from this file. Keep it current.

## Ownership split

| Area                                                                                                                                                                                                                                      | Owner   |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------- |
| `crates/qrtz-lifecycle`, `crates/qrtz-model`, `crates/qrtz-storage`                                                                                                                                                                       | Bozniak |
| `qrtz-llm` (OpenRouter `anthropic/claude-haiku-5.5`, deadlines, capture-aware prompts, extractive fallback): **extractive fallback done** (`LlmSteps::on_device`, ADR 0042; wire it in place of `CoreSteps`, N53), OpenRouter next        | Bozniak |
| `qrtz-search` (index, FTS, CJK/Thai, chat retrieval, MiniLM)                                                                                                                                                                              | Bozniak |
| Pending-delete tombstones (`RuntimeDeps::pending_delete`) and `assert_deletable`: **done** (`qrtz_storage::PendingDeletes`, `NoteRuntime::{schedule_delete, undo_delete, pending_deletes, assert_deletable}`; see rust-core-api.md §6 q8) | Bozniak |
| `crates/qrtz-app` wiring (ports, EventSink, HelperPort, Store, recover, AppDirs, config, secrets)                                                                                                                                         | Bobs    |
| `apps/desktop-tauri/src-tauri` shell (windows, tray, shortcuts, notifications, media protocol)                                                                                                                                            | Bobs    |
| `qrtz-talk` (behind `TalkPort`): todo. `qrtz-backup` (Markdown export, backup zip): **done**, ADR 0043, wiring in rust-core-api.md §9                                                                                                     | Bozniak |
| `qrtz-helper-bridge`, helper protocol, mocks (N38), arbiter mux (N5)                                                                                                                                                                      | Bobs    |
| E2E port to the real Tauri app; removing Electron; memory numbers; docs/ADR/CHANGELOG for the cutover                                                                                                                                     | Bobs    |

Bozniak's pieces plug in behind the `StepExecutor` (summary/index/resummarize/chat) and `NoteRuntime` seams;
qrtz-app never re-implements them. Until qrtz-llm/qrtz-search land, the Tauri build uses `CoreSteps` (real
finalize; summary fails with a clear non-retryable error and the meeting finishes without a summary; index
skipped). Tombstones (f7cb833) are wired: `RuntimeDeps::pending_delete = Some(PendingDeletes::new(data dir))`,
`note:delete {id, deferMs}` → `schedule_delete`, `note:undo-delete` → `{restored: undo_delete}`, `note:list`
hides `pending_deletes()`, and the runtime (not the shell) emits `note:changed`.

## Wiring rules (from docs/rust-core-api.md, pulled at the start of the move)

1. `Clock` needs `mono_ms` (N45); production uses `SystemClock`.
2. Busy errors carry ids; `recording.stop` carries `audioPath`; `recording.abort{audioPath}` before any restart.
3. `note:session` carries `silent`, `noAudio`, `deviceLost`; `note:pipeline` carries `capture`.
4. `note:delete` → `NoteRuntime::delete_meeting` through `NoteRuntimePort` (refuses the live meeting).
5. `note.notifyNoAudio` is in the config (contract-driven); the no-audio OS notification is a non-IPC hook in
   qrtz-app (edge-triggered once per meeting, like `NoteService.emitSession`).
6. `AppDirs::prepare` runs at startup before `Db::open`; the DB file and secrets are 0600, dirs 0700.

## Phases

| #   | Phase                                                                                                                                                                                                                                                              | State | Commit    |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----- | --------- |
| 0   | This file, ownership split                                                                                                                                                                                                                                         | done  | (this)    |
| 1   | `src-tauri` shell in the workspace; qrtz-app: NoteRuntimePort (all runtime channels), StoragePort over `StorageHandle`, EventSink (bus + OS notifications + no-audio hook), HelperPort over the supervisor, recover() at boot, AppDirs, file secrets 0600 on Linux | done  | (see log) |
| 2   | Talk routing only (`talk:*` → `TalkPort`; dictation errors / `permission.revoked` reach talk; `config_changed` on config/secrets set). The service is Bozniak's `qrtz-talk`                                                                                        | done  | (see log) |
| 2b  | Cloud ASR + diarization, offline buffer + backfill (ADR 0044), see below                                                                                                                                                                                           | todo  |           |
| 3   | Integrate Bozniak's `qrtz-llm`, `qrtz-search`, `qrtz-talk`, `qrtz-backup` behind StepExecutor / TalkPort / StoragePort when they land (markdown export done in phase 1)                                                                                            | todo  |           |
| 4   | N38 (mock start reports in the `recording.started` chunk + e2e), N5 (`mux = mic.includes('recording') && state.dictating`, TS + Swift, vectors)                                                                                                                    | todo  |           |
| 5   | E2E suite against the real Tauri app on Linux (WebKitWebDriver/tauri-driver + Rust mock helper): one equivalent per Electron scenario                                                                                                                              | todo  |           |
| 6   | Move the renderer into `apps/desktop-tauri`, delete `apps/desktop` and the TS-only runtime; keep TS spec models + vectors                                                                                                                                          | todo  |           |
| 7   | Memory: Tauri idle/recording vs the last Electron build (Linux); docs, README, ADR (Electron removed), CHANGELOG                                                                                                                                                   | todo  |           |

## Phase 2b: cloud ASR + diarization, offline buffer and backfill (ADR 0044)

History: 07:17 PT on-device FluidAudio diarization + Nemotron ASR; 07:22 PT mic diarized too; **07:26 PT replaced:
no on-device models and no model downloads at all.** Current plan (ADR 0044):

- Primary ASR + diarization: our cloud Nemotron server (`docs/research-nemotron-server.md`); secondary ElevenLabs
  Scribe v2 Realtime (no diarization; AX fusion names speakers). One `StreamingProvider` trait shared with
  Bozniak's `qrtz-talk`. Wire: WebSocket, PCM16 16 kHz binary frames, partial/final JSON.
- Per-channel diarization (`mic:*`, `system:*`, never merged); dominant mic speaker defaults to "me".
- Offline: keep recording, spool unsent frames (`data/asr-spool`, 0700/0600, atomic manifest), cap
  `note.offlineBufferMb` (UI "offline buffer full"), retention `note.offlineBufferDays`; backfill lifecycle with
  retry and crash recovery; fusion re-runs after late words; lostSpans/silences stay in meeting time.
- Talk: clear offline error (Bozniak's qrtz-talk).
- Mine: `crates/qrtz-asr`, helper protocol (`recording.audio`, ASR out of the helper), mocks, wiring, UI states.
  Bozniak: runtime integration (backfill step, merge + fusion re-run, session flags), qrtz-talk.
- Talk/backup ownership (07:21 PT): Bozniak owns `qrtz-talk` and `qrtz-backup`; qrtz-app only routes `talk:*` and
  `data:*-backup`. My parked talk WIP (cleanup port + TS-oracle vectors) is at `/workspace/qrtz-talk-wip` on the box.

## Running the Tauri app on headless Linux

WebKitGTK needs a D-Bus session bus, and tao blocks on `xdg-desktop-portal` activation if the bus can start it.
Use the private bus config (no service dir):

```sh
QRTZ_USER_DATA=/tmp/qrtz-run QRTZ_MOCK_HELPER=1 QRTZ_MULTI_INSTANCE=1 \
  dbus-run-session --config-file=scripts/e2e-dbus.conf -- xvfb-run -a target/debug/qrtz
```

A debug build loads `devUrl` (vite on 1420); build with `--features custom-protocol` to serve `dist/`.
On Linux the tray is off unless `QRTZ_TRAY=1`.

## Mac-only checks (cannot run on the Linux box)

- Swift helper changes compile and pass XCTest on macOS (see docs/mac-verify.md).
- Swift helper without Parakeet/SpeechAnalyzer, streaming `recording.audio` frames (ADR 0044): build, CPU, frame timing on a real call.
- Keychain secret store (`KeychainSecretStore`), TCC prompts, real capture, WKWebView rendering, tray/notifications.

## Log

- 2026-10-17 07:00 PT: started; baseline `cargo test --workspace` 120 passed.
- 2026-10-17 07:20 PT: Phase 1 green: `cargo test --workspace` 138 passed; clippy -D warnings clean; npm lint/typecheck ok; vitest 536 passed / 2 skipped. App boots headless on Linux (data dir 0700, qrtz.db 0600, mock helper Simulated, recovery ran). Next: Phase 2 talk service or Phase 5 E2E harness.
- 2026-10-17 07:25 PT (Bozniak, hardening cycle 13): undo restores only a delete still inside its window (N51,
  `720ae3a`, ADR 0041; TS + Rust); port checklist of every hardening behaviour in `docs/architecture-review/talk-and-note.md` §21.1.
- 2026-10-17 07:25 PT: talk + backup reassigned to Bozniak; talk WIP parked off-repo. Phase 2 = routing. N52 (Keychain index write error is returned + logged) done. Mic diarization correction recorded in Phase 2b.
- 2026-10-17 07:40 PT: qrtz-backup wired per rust-core-api.md §9 (note:export via export_meeting, markdown.rs deleted; data:export-backup / data:import-backup with QRTZ_BACKUP_PATH or dialogs, throttled data:backup-progress, the runtime's PendingDeletes shared with import); qrtz-llm `LlmSteps::on_device` wired in place of CoreSteps for summary (N53). Boot tests: backup round trip into a fresh library, cancel paths, summary done.

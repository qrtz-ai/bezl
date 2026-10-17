# PROGRESS.md: iteration log

Newest entries at the bottom. Times are in America/Los_Angeles (PT).

## 2026-10-16: iteration 0 (setup and research)

- Research recorded in `docs/research-memo.md` and `docs/design-system.md`.
- Wrote PLAN.md, LOOP.md and this file.

## 2026-10-16 ~17:15–17:35 PT: iteration 1 (M0 architecture hardening)

- **Done:** M0.1–M0.8, M0.10 and M0.11.
  - `@qrtz/core` now has: `AppError`, a structured logger with redaction, a versioned zod IPC contract, a router, a typed client and event bus, a sub-app manifest with capabilities and permissions, a Supervisor (backoff, breaker, heartbeat), the helper JSON-lines protocol and a mock, a config schema, a `SecretStore`, and a migration runner.
  - Desktop main is split into `main/*`. The worker is a supervised utilityProcess using `node:sqlite` with FTS5. Secrets use safeStorage. There are crash hooks and security hardening.
- **M0.9:** the error boundary and crash hooks are implemented. The e2e fallback test is still pending.
- **Verified:** lint ✓, typecheck (tsc -b plus a test project) ✓, vitest 34/34 ✓, package ✓, e2e (Playwright, xvfb) ✓. The packaged binary boots and the worker starts.
- **Bugs found and fixed along the way:**
  - Vite stubbed `node:sqlite`, so it's now external.
  - zod 4 rejects duplicate discriminators; the worker protocol now uses a union.
  - `params` must be optional.
- **Note:** Playwright can't drive the fused, packaged app (the inspect fuse is off, by design). The e2e runs the production `.vite` build on the stock Electron binary.
- **Next:** M0.9 e2e test, then M1.1 (tokens).

## 2026-10-16 ~17:35–17:42 PT: iteration 2 (M1.1, M1.2)

- **Tokens:** qrtz.ai neutral-dark palette, Geist/mono type, space, radii, motion and z-index, exposed as CSS vars (`--q-*`) plus global CSS (reset, focus ring, blink/spin/fade keyframes, underline motion, reduced-motion support).
- **Primitives:** Heading (blinking `_` cursor, mono), Button (primary/secondary/ghost/danger, sm/md/lg, loading), Badge (qrtz.ai tinted style), Kbd, Spinner. Home has been restyled.
- **Verified:** lint, typecheck, 36 tests, package and e2e all green. The e2e saves a screenshot to `test-results/home.png`.
- **Next:** M1.3 (inputs).

## 2026-10-16 ~17:42–17:48 PT: iteration 3 (M2.1, M2.5, M2.6, pulled forward because they're pure TS)

- `@qrtz/core/memo`:
  - Zod domain types: Meeting, Segment, AsrEvent, ActiveSpeakerEvent, SpeakerTurn, caption and chat events.
  - `TranscriptAssembler`: partials are replaced and finals sorted; late partials and empty finals are ignored.
  - Diarization fusion: Sortformer slots are matched to AX active-speaker names by greedy one-to-one, overlap-weighted assignment with a min-share threshold. `labelSegments` applies the result.
- These live in core for now. If memo logic grows, consider a `@qrtz/memo-core` package with a boundary rule.
- **Verified:** lint, typecheck, 40 tests, package and e2e all green.
- **Next, in order:** M0.9 (e2e test for the error fallback), M1.3 → M1.7 (DS plus gallery), then M2.2 (memo DB schema and FTS), M2.3, M2.4, M2.7–M2.9.

## 2026-10-16 ~17:34–18:15 PT: iteration 4 (M2 + M3 + M0.9: Memo end to end)

- **M2.2:**
  - Memo schema migration v2 adds meetings, segments, summaries, and FTS5 indexes (`segments_fts` with triggers, plus `meetings_fts`).
  - `MemoRepo` handles CRUD, idempotent segment replacement, cascading deletes, and a sanitized FTS query builder (prefix and porter stemming).
- **M2.3:** the helper protocol now has memo messages:
  - `meeting.detected/ended`, `ax.participants/activeSpeaker/caption/chat`, `audio.level`, `asr`, and `recording.start/stop/started/stopped`.
  - The mock helper plays the scripted "Weekly Product Sync" meeting. A desktop wrapper writes a fixture 16 kHz WAV to the recording path.
- **M2.4:** a pure `sessionReducer` and `SessionMachine` (idle → detected → recording → processing → done/error).
- **M2.7:** the LLM layer covers OpenAI-compatible APIs (OpenRouter is the default and sends `HTTP-Referer`/`X-Title`), Anthropic and Google, plus a deterministic extractive fallback (summary, action items with owners, Q&A). `summarizeMeeting` never throws.
- **M2.8:** Markdown export (golden snapshot test).
- **M2.9:** search through `memo:list {query}`.
- **Backend wiring:**
  - Worker `memo.*` methods (`createWorkerHandlers`) are integration-tested on real `node:sqlite`.
  - `MemoService` in main orchestrates helper events → session → worker → summary.
  - New IPC channels and events: `memo:*`, `permissions:*`.
  - A `qrtz-media://` protocol serves only `recordings/<uuid>.wav`, and the CSP got `media-src`.
- **Direction changes applied:**
  - The re-transcription pass is gone: the protocol's `post.*` messages are removed and the live transcript is final.
  - `nameSegmentsFromAx` does live speaker fusion with lag correction.
  - The default provider is OpenRouter (`google/gemini-2.5-flash`). The env alias `QRTZ_SECRET_OPENROUTER` works.
  - Zoom and Meet are P0. PLAN.md and the research memo are updated.
- **M3 (`@qrtz/feature-memo`):**
  - Permission onboarding.
  - Live HUD with timer, level meter, active speaker, streaming lines and partial text.
  - Detection prompt, processing and done states.
  - Meetings list with FTS search and snippets.
  - Detail view with audio player, synced transcript (click to seek, current segment highlighted), summary, action items, Q&A, Markdown save/copy and delete.
  - The Settings page has provider, model and API key fields (safeStorage), plus system status.
- **UI components added:** Card, Input, Select, Field, Tabs, EmptyState. Text now takes HTML attributes.
- **M0.9:** a jsdom test shows that a crashing sub app renders the fallback and the shell survives.
- **Tests:**
  - **IPC:** capability matrix across real manifests, and a check that no channel's capability is unreachable.
  - **DB and worker:** integration suites.
  - **E2E:** `e2e/memo.spec.ts` drives the full flow: detection → record → live transcript (AX speaker names) → fixture audio written → stop → extractive summary and action items → transcript tab → list and search (hit and miss) → Markdown export file → persistence across a restart.
- **Verified:** lint ✓, typecheck ✓, vitest (all green) ✓, package ✓, e2e 2/2 ✓. Screenshots: `test-results/memo-live.png`, `memo-summary.png`.

### Side work (mocked or blocked, not blocking)

- **Swift helper (M4):** not written yet. The Linux build runs the TS mock with the identical protocol. [MAC-VERIFY]
- **Live summaries:** without an OpenRouter key, summaries come from the extractive fallback. With `QRTZ_SECRET_OPENROUTER` set, the same path calls OpenRouter.
- **Audio player:** on Linux it shows a 0:00 duration until playback starts. Check on macOS.
- **Lint warning:** a `prefer-startsWith` warning remains in `main/security.ts` because the auto-review tool errored on the one-line edit.

### Next

- M4.1–M4.5 (Swift helper: stdio, AX for Zoom and Meet, audio taps, FluidAudio streaming) [MAC-VERIFY].
- M5.1 (resume summaries stuck in processing).
- M6.1 (gallery and the remaining components).

## 2026-10-16 ~17:48–18:25 PT: iteration 5 (M4 Swift helper, M5.1, part of M6)

- **M4: `native/qrtz-helper` (SwiftPM, Swift 6, macOS 14+).**
  - **QrtzHelperCore (Foundation-only):**
    - Protocol mirror with an integral-number JSON encoder.
    - `MeetingClassifier` and `MeetingTracker` (4 s grace), covering Zoom.app, Zoom web, and Meet in Chrome and Safari (with a title fallback). Arc, Edge, Brave and Teams are best-effort.
    - `AXNode` snapshot model, `AXScraper` (participants panel, "X is speaking", Captions / Live Transcript region) and `ScrapeDiffer` (settled captions only).
    - `UtteranceSegmenter` (cumulative transcript → partial and final `asr`), WAV writer, resampler, mixer.
    - `HelperController` actor with ordered audio and ordered host-message streams. Audio and ASR backends are tried in order, with fallback.
  - **Executable `mac/`:**
    - **AX:** windows are polled every second and AXObserver notifications trigger a debounced scrape. Chromium gets `AXManualAccessibility`. Snapshots are bounded by depth and node count. `--dump-ax` records fixtures.
    - **Permissions:** Accessibility, Screen Recording, Microphone.
    - **Audio:** a Core Audio process tap on a private aggregate device plus the AVAudioEngine mic, with ScreenCaptureKit as the fallback.
    - **ASR:** FluidAudio `StreamingUnifiedAsrManager` (Parakeet Unified streaming), with SpeechAnalyzer as the fallback (macOS 26, `#if compiler(>=6.2)`).
  - **Linux:** a stub executable (protocol only, no backends).
- **Verification on Linux (Swift 6.1.2 toolchain in /tmp):**
  - `swift build` and 19 XCTest cases are green.
  - The shared golden fixtures `Fixtures/protocol/*.jsonl` round-trip byte-for-byte in Swift and validate with zod in TS. Both sides enforce full message-type coverage and reject invalid fixtures.
  - `swift-helper.test.ts` spawns the real Swift binary and drives a ping, record and shutdown sequence over stdio.
  - The `mac/` files are syntax-checked only (`swift-format lint`, `swiftc -parse`). [MAC-VERIFY]
- **Packaging:**
  - `npm run build:helper` (macOS: release arm64; elsewhere a no-op) and `npm run test:helper`.
  - Forge `extraResource` includes the helper when it has been built. Info.plist has mic and audio-capture usage strings. `entitlements.mac.plist` added.
  - `QRTZ_HELPER_BIN` overrides the helper path.
- **M5.1:** at startup, `memo.recover` finishes meetings stuck in `recording` or `processing`. It repairs the WAV header (helper killed), derives the duration and summarizes. Integration-tested.
- **M6 (partial):**
  - ⌘⇧R global shortcut toggles recording.
  - Audio retention setting (`memo.retainAudioDays`) with a Settings selector; `memo.pruneAudio` runs at startup. Tested.
- **Lint:** the two remaining warnings are fixed, so lint is clean.
- **Verified:** lint ✓, typecheck ✓, vitest 78 ✓, Swift tests 19 ✓, package ✓, e2e 2/2 ✓.

### Side work

- **macOS:** the whole `mac/` layer needs a first compile and run on a Mac (Xcode 16+). The FluidAudio API was checked against its current source (v0.17.7); the AX heuristics are modelled, not recorded. [MAC-VERIFY]
- **Captions:** Meet and Zoom only expose captions in AX when captions are switched on in the meeting. The helper does not toggle them.
- **Chrome tabs:** a Meet tab in Chrome is found only when it is the window's active tab, because AX exposes only the active tab's web area.
- **System audio TCC:** the process tap uses its own permission ("System Audio Recording Only"), which has no public preflight. Onboarding shows Screen Recording, and the first recording triggers the system-audio prompt.
- **Echo:** without headphones, the mic picks up the remote side again. Echo cancellation (FluidAudio LocalVQE) is a later item.

### Next

- **MAC-VERIFY pass:** build, fix compile errors, record real AX fixtures with `--dump-ax`.
- M5.3 signing and notarization (needs an Apple Developer ID).
- M6.1 gallery and remaining components.
- M6.2 palette.
- M6.3 floating HUD.
- M6.5 chat.

## 2026-10-16 18:02 PT: iteration 6 (rest of M6, echo gate, Mac readiness)

- **M6.1:**
  - New `@qrtz/ui` components: Dialog (Esc, backdrop click, focus handling), ToastProvider/useToast, Tooltip, Switch, TextArea, ListItem.
  - New `@qrtz/feature-gallery` sub app showing color tokens, typography, buttons, badges/Kbd/spinner/tooltip, inputs, tabs and list, dialog and toast, and the empty state.
- **M6.2:** ⌘K command palette in the shell.
  - Fuzzy filtering (pure `filter.ts`, unit-tested) and keyboard navigation.
  - Commands: "Go to <sub app>" and Start/Stop recording, through a shell-scoped IPC client. The shell manifest gained `memo.read` and `memo.record`.
  - The sidebar has a "Search… ⌘K" button.
- **M6.3:** floating, always-on-top mini HUD window (`index.html#hud`).
  - Frameless, shows on all workspaces including full-screen spaces, and opens without taking focus.
  - Shows a pulsing dot, the active speaker, the live line, a clock and a Stop button.
  - Opens on recording or processing and closes 1.5 s after. Controlled by `memo.floatingHud` (on by default).
- **Visual polish (qrtz.ai):** sidebar has a mono "qrtz" wordmark with a blinking block cursor, muted nav that highlights on hover and when active, the ⌘K search field, and a "⌘⇧R record" footer. Checked with xvfb screenshots of the palette, gallery, mini HUD and memo.
- **M6.5:** AX chat scraping for Meet "In-call messages" and Zoom "From X to Everyone", with consecutive messages grouped under one author and timestamps dropped. `ScrapeDiffer` treats the first scrape as history, then emits only new messages as `ax.chat`. Self-name detection from "(You)" / "(me)".
- **Echo:**
  - Shared `MeetingContext`: the AX layer writes the active speaker and self name; audio threads read them.
  - `EchoGate` in the mixer ducks the mic to −20 dB when someone else is the active speaker and the far end is active, or to −10 dB on the energy heuristic when the speaker is unknown. Gain changes are smoothed.
  - Tested in Swift.
- **Mac-readiness pass over the Swift code:**
  - Removed `traits:` from the FluidAudio dependency. It needs tools 6.1, but the manifest declares 6.0.
  - The executable target uses Swift 5 language mode, so non-Sendable SDK types (AVAudioPCMBuffer across the FluidAudio actor, SCStream, AXUIElement) produce warnings, not errors.
  - Added the missing `super.init()` in the ScreenCaptureKitSource init, an explicit `import CoreMedia`, and explicit `JSONValue` in the `--dump-ax` closures.
  - Re-read the code against the current API shapes of CATapDescription, the aggregate device keys, AudioDeviceCreateIOProcIDWithBlock, the SCStream config, AVAudioEngine taps, StreamingUnifiedAsrManager and SpeechAnalyzer.
- **docs/mac-verify.md:** a 10-minute checklist covering build, protocol smoke test, `--dump-ax` for Zoom app/web and Meet in Chrome/Safari, audio and ASR with fallback and echo checks, and the in-app flow.
- **Verified:**
  - lint ✓, typecheck ✓, vitest ✓, Swift 23 ✓, package ✓.
  - e2e 3/3 ✓: smoke; memo, extended with mini-HUD window and always-on-top checks; and a new shell spec covering the palette (open, filter, Enter to navigate, start/stop recording) and the gallery (all sections, dialog with Esc, toast, switch).

### Side work

- The `mac/` Swift layer still needs its first real compile on a Mac (docs/mac-verify.md, step 0). [MAC-VERIFY]
- The echo gate is a heuristic, not AEC.
- The palette doesn't search meetings yet (next: a `memo:list` query inside the palette).

### Next

- Run docs/mac-verify.md on a Mac and fix whatever it finds.
- M5.3 signing and notarization (needs an Apple Developer ID).
- Meeting search in the palette.
- Polish the Home dashboard (recent meetings and onboarding state).

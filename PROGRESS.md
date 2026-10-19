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

### Next

- Run docs/mac-verify.md on a Mac and fix whatever it finds.
- M5.3 signing and notarization (needs an Apple Developer ID).
- Polish the Home dashboard (recent meetings and onboarding state).
- **Follow-up in the same iteration:**
  - The ⌘K palette now searches meetings with FTS through `memo:list`. Picking a result opens it in Memo via core `requestNavigate`/`takePendingMeeting`, so sub apps never import each other.
  - Home hints updated.
  - e2e covers palette meeting search after a restart.

## 2026-10-16 ~18:05–18:25 PT: iteration 7 (Home, LocalVQE AEC, hardening)

- Home dashboard: recent meetings, permission status, quick record. `nav.ts` handles cross-app navigation.
- LocalVQE echo cancellation in MicPipeline, using system audio as the reference. Falls back permanently to EchoGate on any error. DSP.Aligner added. `QRTZ_AEC=0` disables it.
- Fail-soft helper crash: salvages the WAV and the transcript. New `e2e/crash.spec.ts`. Mock fault injection with `QRTZ_MOCK_CRASH_AFTER_MS`.
- Bug fix: startup recovery and the dev seed ran before the worker existed, so M5.1 recovery never ran in-app. Added `WorkerClient.ready()` and the startup jobs now await it.
- Large meetings: transcript rows are memoized with `content-visibility`, plus a binary-search active row. Added the `dev.seedLarge` seed and a 2-hour e2e (open ~0.2 s, search ~0.5 s).
- Accessibility: axe checks over CDP in e2e. Fixed html lang, unlabeled listboxes and inputs, and danger-button contrast.
- Auto-update scaffold (update-electron-app). It stays off unless the build is packaged, the platform is macOS or Windows, and `QRTZ_AUTO_UPDATE=1` is set.
- README Status section.

### Side work

- LocalVQE and real capture are only compiled and unit-tested via the Linux core. Real-call verification needs a Mac.
- In e2e under xvfb, Chromium throttles renderer timers, so the search test pokes the page while polling.

### Next

- A full empty and error states sweep (Settings and chat errors).
- First-run polish with screenshots.
- Virtualize the meeting list once it holds hundreds of meetings.

## 2026-10-16 ~18:25–18:35 PT: iteration 8 (error states, first-run polish, list scale, LLM stub test, Mac compile pass)

- Error states:
  - New `Callout` UI primitive (alert or status role).
  - LLM failure: the summary stores `llmError`, and the detail view shows "AI summary failed" with Retry (`memo:resummarize`). Provider errors show the provider's own message plus a hint for 401/403/429.
  - Helper health in SessionView (ok, simulated, missing, down): a notice on Home and Memo, the record button is disabled, and `memo:start` returns a clear error. A macOS build without the helper binary is now reported as "missing" instead of quietly using the mock.
  - Settings sections load independently, with a Retry callout. The meeting detail shows a load-error state with Try again instead of a spinner that never ends. Memo errors use a dismissible callout.
- First run: once setup is complete and meetings exist, setup collapses to one line below Recent meetings. Settings now has a separate Recordings card. The screenshot walkthrough is `e2e/screens.spec.ts`, and the images are in `docs/screenshots/` and the README.
- Meeting list: paged at 100 (`limit+1` signals there are more). Rows use content-visibility. A 500-meeting e2e loads the next page in about 0.1 s.
  - Bug fixed: the list was silently capped at 100 meetings, so older ones were unreachable.
  - Bug fixed: crash recovery only scanned the newest 1000 meetings; it now selects stuck meetings by status.
- OpenRouter end to end without a key:
  - vitest against a local OpenAI-compatible HTTP stub: auth header, X-Title, path, JSON parsing, a 429 fallback then a successful retry, and a non-JSON reply.
  - e2e with `QRTZ_LLM_BASE_URL`: a 503 fallback, then Retry returns the AI summary.
- Mac compile risk:
  - Fixed a duplicate `LocalVqeEnhancer` class in `mac/Audio.swift`, which would have been a redeclaration error on Mac.
  - FluidAudio is now pinned to 0.17.x. The LocalVQE and StreamingUnifiedAsrManager signatures were checked against the v0.17.7 sources.
  - New `scripts/check-swift.mjs` (part of lint) flags duplicate types and unbalanced `#if` blocks in the Swift sources.

### Side work

- There is no chat UI yet, so "chat errors" have nothing to attach to. The LLM failure and retry path covers summaries. Chat should reuse `Callout` and the same retry pattern.
- Electron ignores `setContentSize` under xvfb, so screenshots are 1024×640.

### Next

- Build on a Mac (docs/mac-verify.md), then real Zoom and Meet runs.
- Chat over meeting transcripts.
- Search results paging.

## 2026-10-16 ~18:34–18:37 PT: iteration 9 (chat, search paging, auto-record, speakers)

- Chat:
  - Core `memo/chat.ts`: term ranking with rarity weighting, a grounded prompt with numbered excerpts, and an extractive quote fallback that also covers LLM failures (`llmError`). Never throws.
  - Worker `memo.chat`: per-meeting retrieval over that transcript, or FTS OR-retrieval across meetings (`repo.searchSegments`).
  - IPC `memo:chat` (memo.read). `ChatPanel` provides the Ask tab and "Ask all meetings", with clickable citations that open the transcript at that moment, plus error and retry.
- Search paging: `memo:list` takes `query` with `limit`/`offset` and uses limit+1 to signal more. "Show 100 more" works for search, and paging resets when the query changes.
- Auto-start:
  - The `memo.autoRecord` config flag defaults to off.
  - When off, detection shows the in-app prompt plus an OS Notification if qrtz is unfocused; clicking the notification starts recording.
  - "Always record" on the prompt goes through a scoped `memo:set-auto-record` (memo.record), so there's no broad config.write capability. There's also a Settings select.
- Speakers: `repo.renameSpeaker` updates the segments and participants in one transaction. Merge renames onto an existing name. IPC `memo:rename-speaker` (memo.write). Speaker chips in detail offer Rename and "Merge into…".
- Tests:
  - vitest (stub OpenRouter): chat per meeting and across meetings, the fallback, rename/merge, and search paging.
  - e2e `chat.spec.ts`: chat, citation seek, rename and merge, the Always-record prompt persisting and auto-starting on the next launch, the Settings toggle, and paged search over 500 meetings.

### Side work

- Retrieval is lexical (FTS plus term overlap). Embeddings could come later if recall is weak on paraphrases.

### Next

- Run an a11y axe pass over the chat and speaker editor.
- Mac build and real-call verification.

## 2026-10-16 ~18:38–18:44 PT: iteration 10 (a11y of chat, tray, semantic retrieval, self-review)

- a11y: the axe helper was extracted to `e2e/axe.ts`. The chat spec now scans the Ask tab, the speaker editor and Ask-all. Fixed `label-content-name-mismatch` on speaker chips; the aria-label now starts with the visible text.
- Tray: the pure `tray-model.ts` (unit-tested) covers icon, menu-bar title (elapsed time), tooltip and menu for each phase and helper state. `tray.ts` uses generated template PNG icons (data URLs, 1x/2x), updates on `memo:session` and ticks every second while recording. The screens e2e runs with `QRTZ_TRAY=1`.
- Semantic retrieval:
  - Migration 3 adds `segment_vectors`.
  - Core `embeddings.ts` does reciprocal-rank fusion of lexical and cosine scores.
  - The worker's `transformersEmbedder` lazily loads Xenova/all-MiniLM-L6-v2 q8 into `userData/models` (`QRTZ_EMBEDDINGS=0` disables it, `QRTZ_EMBEDDINGS_OFFLINE=1` forbids downloads).
  - `memo.chat` indexes on demand. Across meetings it adds the 40 nearest lines that full-text search missed.
  - Background `memo.indexVectors` runs after each meeting. If the embedder fails, retrieval falls back to full-text permanently and logs it.
  - Tests: a fake concept embedder proves a paraphrase is found when FTS finds nothing, plus the fallback. A real MiniLM test is opt-in (`QRTZ_TEST_EMBEDDINGS=1`) and passes in about 1.4 s. The model was verified loading inside Electron's utilityProcess during e2e.
- Self-review:
  - Security drift fixed: dev hooks are now stripped in packaged builds (`DEV_ONLY_ENV`). This covers `QRTZ_LLM_BASE_URL`, which could otherwise send the API key elsewhere. The dev seeds are again unpackaged-only.
  - Removed the dead `CaptionEvent` and `ChatEvent` schemas.
  - `docs/architecture.md` has a new as-built system overview, and `docs/security.md` is updated. Migration names are consistent (`3_segment_vectors`).

### Side work

- Packaged builds don't include transformers.js or onnxruntime-node (the Forge Vite plugin ships only `.vite`), so packaged chat uses full-text retrieval. Fix: copy the module and asar-unpack the `.node` addon in forge `packageAfterCopy`. That needs a Mac check, since the darwin-arm64 onnxruntime binary has to be included.
- Tray on Linux is opt-in, because tray support differs between desktops.

### Next

- Bundle onnxruntime for macOS (the packaging hook above).
- Mac build and real-call verification.

## 2026-10-16 ~18:45–18:50 PT: iteration 11 (bundle embeddings, release readiness)

- Packaging: a Forge `packageAfterCopy` hook in `forge.config.mts` copies the embedding runtime into `app/node_modules`:
  - The transformers.js node entry points.
  - onnxruntime-common.
  - onnxruntime-node dist plus only `bin/napi-v6/<platform>/<arch>`, without the CUDA and TensorRT providers.
  - A `sharp` stub. transformers imports sharp statically for image pipelines we don't use.
  - `asar.unpack` covers `*.node`, `*.dylib` and `*.so*`. Linux x64 resources come to 48 MB (onnxruntime-node alone is 548 MB unpruned).
- Verification: `npm run smoke:packaged` launches `out/qrtz-linux-x64/qrtz` with `QRTZ_EMBED_SELFTEST=1`. The worker embeds a sentence and logs `embedder selftest`, and the script asserts 384 dimensions. Passes 4 of 4 runs (~0.2 s with a cached model).
- Release: `docs/release.md` is the checklist. `forge.config.mts` has env-gated `osxSign` (hardened runtime and entitlements on every nested file) and `osxNotarize` (keychain profile or Apple ID). `scripts/release-mac.sh` runs the checks, the helper build, the signed make, codesign/spctl/stapler verification and DMG notarization.

### Side work

- onnxruntime-node 1.30 has no darwin-x64 binary, so Intel Mac builds fall back to FTS-only retrieval. Recommendation: ship arm64-only.
- Auto-update: update.electronjs.org serves only public repos, and qrtz-ai/qrtz is private. Needs a public releases repo or a static feed (docs/release.md). The `QRTZ_AUTO_UPDATE` default should flip before the first public release.
- The smoke test reuses the model cache at `/tmp/qrtz-e2e-models`. A cold run downloads ~23 MB from Hugging Face.

### Next

- Mac: run `scripts/release-mac.sh` with a Developer ID, and verify real calls (docs/mac-verify.md).
- Pick the auto-update feed.

## 2026-10-16 ~18:49–19:00 PT: iteration 12 (WASM embeddings, update feed, AX fixtures)

- WASM fallback: `chooseBackend()` in `src/worker/embedder.ts` tries native onnxruntime-node first. If that's missing, or `QRTZ_EMBEDDINGS_BACKEND=wasm` is set, it aliases `onnxruntime-node` to onnxruntime-web's Node build (1 thread, wasm loaded from asar-unpacked) before importing transformers.js. The vectors are identical. The Forge hook bundles `ort.node.min.*` plus `ort-wasm-simd-threaded.{mjs,wasm}`; Linux resources went from 48 to 62 MB.
- Verified on the packaged Linux binary:
  - The smoke script passes with the native backend and with the forced WASM backend.
  - With the native `bin/` deleted, auto mode picks WASM. This is the Intel-Mac simulation.
  - Unit tests cover `chooseBackend`.
- Update feed:
  - `resolveUpdateSource` uses a static feed when `QRTZ_UPDATE_FEED_URL` is baked in through a Vite define. It supports `{platform}`/`{arch}` placeholders and rejects non-HTTPS URLs. The default stays off.
  - The zip maker writes `RELEASES.json` when the feed is set.
  - `release-mac.sh` takes the feed and turns updates on. The decision is documented in docs/release.md.
  - No repos or accounts were created.
- AX scrapers:
  - 11 synthetic fixtures (JSON in `--dump-ax` shape), checked by `AXFixtureTests`. 15 assertions failed before the fixes; all pass now.
  - Fixes: toolbar controls ("Show Captions", "Turn on captions") are never taken for panels. A real list is preferred over a labelled group.
  - Speaking indicators are read only from non-text nodes, so caption prose like "we're talking…" isn't mistaken for one. Added the "Name, Active Speaker" form.
  - Layout-only wrappers around caption turns are unwrapped, and Zoom transcript timestamps are dropped.
  - The self name comes from the row when "(You)" is a separate text node.
  - Added de/fr/es labels, speaking verbs and self markers.
  - There is no TS mirror of the scrapers; the TS side only consumes `ax.*` events.

### Side work

- The fixtures are synthetic. Real dumps from current Zoom and Meet builds (`qrtz-helper --dump-ax`) still need to be recorded on a Mac.
- The WASM backend runs single-threaded, because Node can't load ort-web's blob: worker URLs. That's fine for a 384-d MiniLM.

### Next

- Mac: record real AX dumps, run the signed release, verify real calls.

## 2026-10-16 ~18:53–19:25 PT: iteration 13 (live notes, templates, backup, privacy)

- Migration 4 (`notes_templates`) adds the `notes` table (`at_ms` = recording offset) and `meetings.template`.
- Live notes:
  - `Notes.tsx` is used in the live HUD (compact) and the meeting detail's Notes tab: add with Enter, click to edit, empty to delete.
  - `memo:add-note` without a meetingId targets the recording and is stamped by main.
  - `summarizeMeeting(..., { notes, template })`: the LLM prompt includes "My notes" plus merge instructions. The extractive fallback leads with "Your notes: …", boosts note words, and turns TODO / `[ ]` notes into action items. The Markdown export has a "My notes" section.
- Templates: `templates.ts` has General, 1:1, Standup, Sales call and Interview, each with LLM guidance and extractive focus words. Settings has a default; the detail view has a select (`memo:set-template` stores and re-runs) and Re-run. Summaries record the template they used.
- Backup: `main/backup.ts` (adm-zip) writes a zip of a `VACUUM INTO` snapshot, the recordings and `manifest.json` (sha256 and size per file).
  - Import rejects anything not listed, unsafe names, hash/size mismatches and corrupt entries.
  - The worker's `data.importDb` runs `PRAGMA integrity_check` and a schema-version check, then merges meetings that aren't present (transcript, summary, notes, template; audio paths remapped).
  - e2e: export, import into a fresh profile, re-import skips duplicates, a tampered zip is rejected.
- Privacy:
  - `config.privacy { localOnly, excludedApps, pauseHotkey }`. `effectiveLlmProvider` forces `none` for cloud providers in local-only mode (Ollama stays allowed); `isExcludedApp` filters detection.
  - New helper protocol messages `recording.pause` / `recording.resume`, implemented in Swift (silence written, recogniser not fed; test added), the TS mock and the golden fixtures. Main also drops in-flight ASR while paused.
  - The global pause shortcut is re-bound when the setting changes. There's a HUD Pause/Resume button.
- Tests: unit (notes, templates, prompt, import merge, zip integrity, privacy helpers) and e2e (`e2e/notes-privacy.spec.ts`, 3 tests).

### Side work

- Backups are built in memory (adm-zip), which is fine for typical libraries. Multi-GB audio libraries should move to streaming (yazl/yauzl).
- The floating mini HUD (56 px) has no notes field; notes live in the in-app HUD and the meeting detail. A future option is expanding the mini HUD on click.
- The pause hotkey can't be pressed under xvfb, so the e2e covers the HUD button and saving the setting. The Swift test covers helper behavior.

### Next

- Mac verification of pause with real capture, and a streaming backup for large libraries.

## Iteration 14 (2026-10-16)

- Streaming backup: yazl/yauzl replace adm-zip. Files get a per-file sha256 while streaming. Bad archives are rejected with nothing written. A 304 MB test keeps RSS growth under 200 MB.
- Mini HUD notes: a ✎ toggle expands the HUD to 220 px for a notes list and input (`hud:set-expanded`).
- Design system: light/system theme (opt-in, dark stays the default), motion and focus tokens, a global focus ring, a generated docs/design-tokens.md with a drift and contrast test, new gallery sections (callouts, cards, space, motion, focus), and axe-clean gallery in dark and light.
- Startup: an e2e cold-start test with a 1.5 s budget (`QRTZ_COLD_START_BUDGET_MS`) measures about 0.65–0.70 s to home and about 0.2 s to first contentful paint on Linux under xvfb. Main now lazy-loads backup (yazl/yauzl) and updates. Sub-apps were already React.lazy.
- Side work: none blocking. The cold-start number was measured under xvfb, not on a real Mac. docs/design-tokens.md is excluded from oxfmt because it is generated.

## Iteration 15 (2026-10-16)

- Light mode on every view: `e2e/themes.spec.ts` takes screenshots of home, memo list, summary, transcript, gallery, settings and the palette in dark and light (saved to `test-results/themes/`) and runs axe on each. Everything passes with no changes to the views; the iteration 14 status-color fix was enough.
- Backup and restore progress: a throttled `data:backup-progress` event reports checksum, write, verify, import and done. Settings shows a progress bar and percentage. `@qrtz/ui` gained a `Progress` component, which is also in the gallery. A unit test checks that progress only goes up and ends at the total.
- Test audit: `docs/test-matrix.md` maps every flow to its tests. New `e2e/flows.spec.ts` covers the gaps: deleting a meeting (survives restart), Copy Markdown, settings surviving a restart, and the API key never landing in plaintext.
- Bug fixed: Copy Markdown used `navigator.clipboard` and swallowed errors, so it said "Copied" even when the clipboard was denied (for example, when the window wasn't focused). Main now writes the clipboard.
- Fresh-clone check: found one real break. Electron 44 downloads its binary lazily on first launch, so parallel e2e workers raced it in a clean clone (ETXTBSY, "Electron failed to install correctly"). Fixed with a root `postinstall: install-electron`. `scripts/fresh-clone-check.sh` now passes end to end (21 e2e).
- Side work: Linux has no keyring under xvfb, so the e2e checks that saving a key is refused safely there; the save and remove happy path only runs on macOS. Small e2e backups finish too fast to show the progress bar, so progress is checked by the unit test.

## Iteration 16 (2026-10-16)

- Swift tests: they never stopped running. All 29 XCTests passed every time. Swift 6 also runs an empty swift-testing pass whose last line reads "0 tests passed", and that was the line we were reading. `scripts/build-helper.mjs` now:
  - finds the toolchain on PATH, in `QRTZ_SWIFT_TOOLCHAIN`, or at the /tmp location from LOOP.md;
  - runs XCTest only (`--disable-swift-testing`) and counts the results;
  - fails if 0 tests ran or any failed, and prints "N Swift tests passed";
  - fails when the toolchain is missing under `CI`, `QRTZ_REQUIRE_SWIFT=1` or on macOS (elsewhere it warns that it skipped).
- Delete: a confirm dialog, then the meeting is hidden at once with an Undo toast; the delete runs after 6 s. Toasts can now carry an action.
- Mini HUD light mode: found a real bug. The HUD window ignored the theme setting. A shared `useConfigTheme` hook now drives both windows. A new e2e test takes a screenshot and runs axe on the light HUD.
- Backup progress e2e: a 400 MB seeded recording shows the bar (percentage and step) and then the result.
- README rewritten as a landing page: pitch, screenshots, Mac quickstart, what it does, architecture diagram, status, Mac verification, accounts table.
- Side work: if you quit qrtz within 6 s of deleting a meeting, the delete is dropped (the safe direction).

## Iteration 17 (2026-10-16)

- Durable undoable deletes: main writes a tombstone (`pending-deletes.json`, written atomically) as soon as you confirm, and runs the delete after the 6 s undo window. Lists hide tombstoned meetings. If qrtz quits or crashes first, the delete is completed on the next launch once the worker is ready. Covered by `PendingDeletes` unit tests (timer, undo, too-late undo, recovery, failed delete keeps the tombstone, corrupt file) and an e2e test that quits inside the undo window.
- `npm run screenshots` packages the app, runs the screenshot specs (under xvfb on Linux) and refreshes `docs/screenshots/`, including the new light variants. It fails if an expected image is missing.
- Speaker fusion quality pass (`fusion.ts`, 7 new unit tests):
  - AX placeholders ("Unknown", "Guest", "…", blank) end the previous speaker's interval but never vote, so unknown dial-ins stay unnamed instead of borrowing a neighbour's name.
  - Names are normalized (whitespace, zero-width characters) and repeated names merge.
  - Slot mapping now compensates for AX highlight lag (`lagMs`, default 300).
  - Time where two people talk at once counts half, so someone who only interjects over another speaker can't take that speaker's name.
  - Live naming keeps a segment's existing label when AX evidence covers less than 25% of it.
- Fresh-clone check run again at the end (see the report).

## Iteration 18: v0.1.0 production-readiness sweep (2026-10-16)

- `npm audit --omit=dev`: 0 vulnerabilities. Dev: vitest 3→5 and oxfmt 0.41→0.73 clear the critical tinypool advisories, and the tree was reformatted with the new oxfmt. One high advisory remains: `image-size` under `@electron-forge/maker-dmg` → appdmg. It's macOS build time only, only parses our own icon, and has no upstream fix.
- `LICENSE` is proprietary (all rights reserved), and every package's `license` is now `UNLICENSED`, replacing the leftover scaffold "MIT". Jared should confirm this.
- `THIRD_PARTY_NOTICES.md` covers the models (Parakeet EOU under the NVIDIA Open Model License; Parakeet 0.6B and Sortformer v2 under CC-BY-4.0; MiniLM under Apache-2.0), FluidAudio (Apache-2.0), onnxruntime (MIT), Electron and transformers.js, plus a generated table of npm production dependencies (`scripts/third-party.mjs`). Models download at runtime and are not bundled. The packaged app ships only a JS stub of sharp, not the LGPL libvips binaries.
- `CHANGELOG.md` v0.1.0; all packages bumped to 0.1.0; tag `v0.1.0` pushed (tag only, no GitHub release).

## Closing summary

qrtz v0.1.0 is feature-complete for the Memo MVP and green on Linux on every push. That covers lint, typecheck, about 130 unit tests, the packaged smoke test, 24 Electron e2e tests (axe-clean in dark and light), 29 Swift tests, and a from-scratch fresh-clone check. Everything that touches real macOS capture is built but unverified on hardware.

## Redesign: Geist Pixel, minimal two-pane UI, toggles, UI purity (2026-10-16)

- Geist Pixel is the only font. The type scale is on its pixel grid, and typography is lint-enforced outside `@qrtz/ui`. Raw typography outside `packages/ui` went from 110 usages to 0.
- Every visual component lives in `@qrtz/ui`. The shell and features compose `Stack`, `AppFrame`/`SideNav`, `SplitView`, `MeetingRow`, `TranscriptRow`, `NotesEditor`, `SummaryBlock`, `RecordingBar`, `CommandPalette` and the rest. `scripts/check-ui-purity.mjs` blocks `style`, `className`, raw elements, token styling and CSS outside `packages/ui`.
- Home is removed, with bottom tabs and a two-pane Memo. Gallery is dev only. The UI no longer has blurbs.
- Settings has 10 toggles for every automatic behavior, wired in main and the worker (tray live toggle, login item, update gate, detect-prompt gate, summarize and indexing gates).
- OpenRouter free-tier fallback, covered by 4 unit tests, plus an opt-in live test. The live call was not run this session: `/home/box/.secrets/openrouter_qrtz.key` is not on the box (the reset wiped it).

## Lifecycle merge, Feather icons, Memo → Note (2026-10-16)

- Rebased the redesign onto the lifecycle commits (16297e0, c0884b1). `LiveTranscript` merged into their `StreamingTranscript` in `@qrtz/ui`; `PipelineSteps` and `CaptureStatus` restyled in token style. The test IDs `live-transcript`, `live-segment`, `live-partial`, `jump-to-live`, `pipeline-step-*`, `pipeline-retry-*` and `lifecycle-error` are kept, and the meeting view defaults to Transcript.
- Feather icons through one `Icon` (size tokens), with every glyph replaced. Lint blocks icon libraries and glyph icons outside `@qrtz/ui`, and the gallery has an Icons page.
- The sub app is **Note** (product name "qrtz note") everywhere: package, components, IPC, capabilities, settings key (with `memo` → `note` config migration) and tests. Historical entries in this file and in the v0.1.0 changelog still say Memo.

## Backlog for the next session (in priority order)

1. **Mac verification** (`docs/mac-verify.md`): build the helper with Xcode, check the TCC prompts, real ScreenCaptureKit/Core Audio capture, FluidAudio ASR, diarization and echo cancellation on live Zoom and Meet, and the Keychain key path. Fix whatever the Mac compiler flags in `Sources/qrtz-helper/mac`.
2. **Signed release:** get the Apple Developer ID and notarization credentials, run `scripts/release-mac.sh`, and smoke-test the notarized DMG on a clean Mac.
3. **Model licensing:** confirm the LocalVQE license on its model card. Show model attributions in Settings → About (the NVIDIA Open Model License and CC-BY-4.0 require attribution).
4. **Real AX captures:** replace the synthetic fixtures with `--dump-ax` captures and tune the `fusion.ts` settings (lag, minShare) against real speaker data.
5. **Update feed:** choose a host (R2, S3 or GitHub Releases) and turn on `QRTZ_UPDATE_FEED_URL` for signed builds.
6. **Dev dependencies:** swap or patch the `image-size` path in maker-dmg when upstream fixes it.
7. **Live OpenRouter check:** put the key back at `/home/box/.secrets/openrouter_qrtz.key` and run `QRTZ_TEST_OPENROUTER=1 npx vitest run packages/core/src/note/openrouter.test.ts`.
8. **Polish:** README screenshots from a Mac, onboarding copy, and a first-run model download progress indicator.

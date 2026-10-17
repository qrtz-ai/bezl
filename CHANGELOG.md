# Changelog

## Unreleased: redesign

- **One font:** Geist Pixel (Square, OFL-1.1, bundled locally) for everything, on a pixel-grid type scale (display, h1, h2, h3, bodyLg, body, bodySm, caption, label, mono, code; nothing below 13.158px). Typography lives only in `@qrtz/ui`, through `Text`, `Heading`, `Label`, `Code`/`Mono` and `Link`.
- **Layout:** the sidebar and Home dashboard are gone. Tiny bottom tabs (note, settings; gallery only in dev builds) sit under a full-window content area. Note has two panes: the meeting list with Record on the left, and the selected meeting on the right (transcript first; summary, notes and ask one tab away). Secondary actions are in a "more" menu, and search is behind a toggle and ⌘K.
- **Less text:** help blurbs, subtitles, hints and empty-state body copy are removed, and errors are one line.
- **Toggles:** every automatic behavior has its own switch: detect meetings, record automatically, ask to record, floating HUD, delete old recordings, summarize after meetings, index for search, menu bar icon (live), open at login and check for updates.
- **OpenRouter free tier:** with the default model, a 402 or a credits error falls back to a current `:free` model (`google/gemma-4-31b-it:free`, then `openrouter/free`). There is also an opt-in live test (`QRTZ_TEST_OPENROUTER=1`).
- **Memo is now Note (qrtz note):** the sub app, tab (`note`), package (`packages/features/note`, `@qrtz/feature-note`), core module (`packages/core/src/note`), components (`NoteView`, `NoteService`, `NoteRepo`), IPC channels (`note:*`, worker `note.*`), capabilities (`note.read`/`note.record`/`note.write`), settings key (`config.note`) and test IDs (`note-*`, view root `note-app`) are renamed. Stored `config.json` files with a `memo` block are migrated to `note` on first launch and rewritten once. The database holds no sub-app names; the shipped migration label `2_memo` stays as is.
- **Icons:** every icon is a Feather icon (`react-feather` 2.0.10, MIT) through one `Icon` component in `@qrtz/ui` with size tokens (`tokens.icon`). All glyphs (■ ▶ ❚❚ ⌕ ↻ ✎ ☐ ● ◎ ⚙ ▦ …) are replaced, `check-ui-purity` blocks icon-library imports and glyph icons outside `packages/ui`, and the gallery has an Icons page.
- **Lifecycle merge:** the other worker's recording lifecycle (pipeline steps with retry, streaming partials, reconnect/degraded states) is integrated into the redesign: `StreamingTranscript`, `PipelineSteps` and `CaptureStatus` in `@qrtz/ui`, the live transcript shown in the meeting view, and the meeting view opening on Transcript. The summarize and index toggles skip their pipeline steps; a manual Summarize still runs.
- **Lint:** `check-typography.mjs` and `check-ui-purity.mjs` fail on raw typography, `style`/`className`, raw JSX elements, token styling or CSS outside `packages/ui`. Every new component has a gallery entry, and the gallery moved into `@qrtz/ui/gallery`.

## v0.1.0 (2026-10-16)

First tagged build. Verified on Linux with the mock helper; macOS capture still needs on-device verification (see docs/mac-verify.md).

### Added

- **Shell:** Electron, React 19 and Vite, with lazy-loaded sub-apps (Home, Memo, Gallery, Settings), a command palette (⌘K), a menu bar item and capability-checked IPC with a zod contract.
- **Memo:** calendar-free Zoom/Meet detection through the Accessibility tree, a record prompt or "Always record", a live streaming transcript with AX speaker names, and a floating mini HUD (pause, notes, stop) with a ⌘⇧R shortcut.
- **Notes and summaries:** Granola-style timestamped live notes, and summaries with action items and Q&A in 5 templates. They use OpenRouter, Ollama, or the on-device extractive fallback.
- **Archive:** SQLite FTS5 search, semantic search (on-device MiniLM; native onnxruntime with a WASM fallback), chat per meeting or across meetings with citations, speaker rename and merge, Markdown export and Copy, and delete with a confirm dialog and an Undo that survives quitting.
- **Privacy:** pause capture (⌘⇧P), excluded apps, local-only mode and audio retention.
- **Backup and restore:** a streamed zip (yazl/yauzl) with a sha256 manifest and a progress bar. Import verifies everything and never overwrites.
- **Swift helper:** JSON-lines protocol (golden fixtures), AX scrapers for Zoom and Meet (11 fixtures), Core Audio and ScreenCaptureKit capture, FluidAudio ASR, diarization and echo cancellation.
- **Design system (`@qrtz/ui`):** tokens with a generated reference, dark (default), light and system themes, motion and focus tokens, and a component gallery. Every view is axe-clean in both themes.
- **Release tooling:** `scripts/release-mac.sh` (sign, notarize, staple), an optional self-hosted update feed (off by default), `npm run screenshots` and `scripts/fresh-clone-check.sh`.

### Reliability

- Helper crash recovery keeps the audio and transcript, an LLM failure falls back with Retry, and startup recovery finishes interrupted meetings.
- Cold start is about 0.65 s to home on Linux; a 2-hour meeting opens in about 0.2 s; a 400 MB backup runs in bounded memory.

### Security

- Context isolation, sandbox, strict CSP and Electron fuses. Secrets are stored only through safeStorage or `QRTZ_*` env vars, and dev-only env vars are stripped from packaged builds.
- `npm audit --omit=dev`: 0 vulnerabilities. Dev-only: vitest was upgraded to v5, and oxfmt was upgraded. One advisory remains in `image-size` through the macOS DMG maker (build time only, with no fix upstream).

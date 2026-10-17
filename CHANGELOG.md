# Changelog

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

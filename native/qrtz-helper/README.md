# qrtz-helper

The native macOS side of qrtz. Electron spawns it with `--stdio` and talks newline-delimited JSON (see `packages/core/src/helper/protocol.ts`). Shared golden fixtures live in `Fixtures/protocol/`. zod (vitest) and `ProtocolGoldenTests.swift` both check them.

| Piece             | Implementation                                                                                                                                                                                                 | Fallback                                                        |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| Meeting detection | AX window scan, 1 s poll plus AXObserver for Zoom.app (`us.zoom.xos`), Zoom web (`*.zoom.us/wc/…`) and Meet (`meet.google.com/xxx-xxxx-xxx`) in Chrome and Safari. Arc, Edge, Brave and Teams are best-effort. | Meet in Safari is detected from the title when there's no AXURL |
| Scraping          | `AXScraper` heuristics on an `AXNode` snapshot: participants panel, "X is speaking" labels, Captions / Live Transcript region                                                                                  | None                                                            |
| System audio      | Core Audio process tap and a private aggregate device (macOS 14.2+)                                                                                                                                            | ScreenCaptureKit audio stream                                   |
| Mic               | AVAudioEngine input, mixed with system audio                                                                                                                                                                   | System audio only                                               |
| ASR (live, final) | FluidAudio `StreamingUnifiedAsrManager` (Parakeet Unified 0.6B, ANE)                                                                                                                                           | Apple SpeechAnalyzer (macOS 26)                                 |
| Recording         | 16 kHz mono 16-bit WAV; the header is finalized on stop. If the helper is killed, the app repairs the header at next launch (M5.1).                                                                            | None                                                            |

```sh
# macOS (Xcode 16+): release arm64 build that Forge bundles into Contents/Resources
bun run build:helper
# any platform with a Swift 6 toolchain: builds the core + stub executable and runs the tests
bun run test:helper
# macOS: dump the AX trees of open meeting windows as JSON (for new scraper fixtures)
.build/release/qrtz-helper --dump-ax > meet.jsonl
```

On Linux, `QrtzHelperCore` (protocol, classifier, scrapers, segmenter, WAV, controller) builds and passes all tests. The executable builds as a stub that has the protocol but no capture backends. The `mac/` sources are only syntax-checked there (`swift-format lint`, `swiftc -parse`) and need a Mac to type-check. [MAC-VERIFY]

You can point the app at any helper build with `QRTZ_HELPER_BIN=/path/to/qrtz-helper`.

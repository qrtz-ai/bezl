# Mac verification checklist (about 10 minutes)

Runs on an Apple Silicon Mac with macOS 14.2+ and Xcode 16+ (`xcode-select -p`). macOS 26 is needed to test the SpeechAnalyzer fallback.
The Swift helper's portable core is tested on Linux. Everything below covers the parts only a Mac can run.
Mark each box, and paste any compiler error or odd output into an issue (or just tell Grok Bot).

## 0. Build (2 min)

```sh
git pull && npm ci
npm run build:helper           # swift build -c release --arch arm64 (fetches FluidAudio)
npm run test:helper            # prints "29 Swift tests passed" (fails on 0)
native/qrtz-helper/.build/release/qrtz-helper --version   # → 0.1.0
```

- [ ] `build:helper` compiles. If it fails, the error is most likely in `Sources/qrtz-helper/mac/*.swift`, which Linux can't type-check.
- [ ] `test:helper` passes.

## 1. Protocol smoke test (1 min)

```sh
printf '{"type":"ping","id":"a"}\n{"type":"permissions.check"}\n{"type":"shutdown"}\n' \
  | native/qrtz-helper/.build/release/qrtz-helper --stdio
```

- [ ] The output starts with `hello` and has a `features` list that includes `ax`, `audio.process-tap` and `asr.parakeet`. Then `permissions.status` and `pong`, and the helper exits.

## 2. Accessibility scraping (3 min)

Give your terminal Accessibility permission (System Settings → Privacy & Security → Accessibility). Join a test call, turn on captions, and open the participants and chat panels.

```sh
native/qrtz-helper/.build/release/qrtz-helper --dump-ax > /tmp/ax.jsonl
jq -c '{bundleId,title,url,meeting,participants,activeSpeaker}' /tmp/ax.jsonl
```

- [ ] **Zoom.app:** `meeting` is `zoom-app:<pid>`, and participants are listed.
- [ ] **Meet in Chrome:** `meeting` is `meet:abc-defg-hij` and `url` is filled in.
- [ ] **Meet in Safari:** `meeting` is detected, from the title if `url` is null.
- [ ] **Zoom web (app.zoom.us/wc/…) in Chrome:** `meeting` is `zoom-web:<id>`.
- [ ] `activeSpeaker` is filled in while someone talks. This one is the most likely to need a heuristic fix.
- [ ] Commit `/tmp/ax.jsonl` as `native/qrtz-helper/Tests/Fixtures/ax/<app>.jsonl` (remove private names first). These become scraper regression fixtures.

## 3. Audio and live transcription (3 min)

```sh
mkdir -p /tmp/q && (printf '{"type":"recording.start","audioPath":"/tmp/q/r.wav"}\n'; sleep 25; \
  printf '{"type":"recording.stop"}\n{"type":"shutdown"}\n') \
  | native/qrtz-helper/.build/release/qrtz-helper --stdio | tee /tmp/q/out.jsonl | jq -c 'select(.type!="audio.level")'
```

Play a YouTube talk and speak for a few seconds.

- [ ] The first run prompts for System Audio Recording and Microphone. The Parakeet models (about 600 MB) download once, so the first run may take a minute.
- [ ] The log shows `audio: core-audio-tap` and `asr: parakeet-unified-streaming`.
- [ ] `asr` partial and final events arrive within about 2 s of speech, for both the video and your voice.
- [ ] `afplay /tmp/q/r.wav` plays both sides, and `recording.stopped.durationMs` is about 25000.
- [ ] **Fallback check:** deny System Audio Recording. The log should show `audio core-audio-tap failed …` followed by `audio: screencapturekit`.
- [ ] **Echo check:** without headphones, during a call, remote speech should not appear twice in the transcript. The mic is ducked while AX shows someone else as the active speaker.

## 4. Inside the app (1 min)

```sh
npm run package && open apps/desktop/out/qrtz-darwin-arm64/qrtz.app
```

- [ ] `Contents/Resources/qrtz-helper` exists in the .app, and Settings → System shows the helper as connected and not mock.
- [ ] Join a Zoom or Meet call. qrtz offers to record, the floating HUD appears (top right, above Zoom), ⌘⇧R stops recording, and a summary appears.

## Known gaps

- **No real echo cancellation:** the echo gate is a heuristic. The upgrade path is FluidAudio LocalVQE AEC with the system tap as the far-end reference.
- **Background Chrome tabs:** a Meet tab is only seen when it is the active tab in its window, because AX exposes only the active tab's web area.
- **Captions:** captions need to be switched on in Zoom or Meet for `ax.caption`. The transcript itself comes from Parakeet and does not depend on them.

## qrtz talk (dictation, 10 min)

- [ ] `mac/Talk.swift` compiles (Linux can't type-check it). Two Parakeet instances are loaded (meeting + dictation), so check memory.
- [ ] Hold **fn** in TextEdit and speak: the pill shows level and streaming text, and on release the text appears at the caret (`insert.result` method `ax-selected-text`).
- [ ] Chrome/Slack/VS Code (Electron fields often ignore AX writes): text arrives via `paste`, and the clipboard you had before is restored.
- [ ] Input Monitoring / Accessibility denied: a `dictation.hotkey` error is logged and ⌘⇧Space still works.
- [ ] Warm mic on: the first word isn't clipped (pre-roll), and the mic indicator stays on as expected.
- [ ] With an ElevenLabs or Deepgram key: the history meta shows the engine. Pull the network mid-dictation and it shows "on-device (fallback)".
- [ ] Record `talk latency` log lines (firstPartialMs, finalMs, insertedMs) for each engine, and run `node scripts/bench-talk-asr.mjs speech16k.wav 5`. Put the numbers in docs/research-talk.md §2.

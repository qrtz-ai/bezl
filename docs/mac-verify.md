# Mac verification checklist (about 10 minutes)

Runs on an Apple Silicon Mac with macOS 15+ and Xcode 16+ (`xcode-select -p`). macOS 26 is needed to test the SpeechAnalyzer fallback.
The Swift helper's portable core is tested on Linux. Everything below covers the parts only a Mac can run.
Mark each box, and paste any compiler error or odd output into an issue (or just tell Grok Bot).

## 0. Build (2 min)

```sh
git pull && bun install --frozen-lockfile
bun run build:helper           # swift build -c release --arch arm64 (fetches FluidAudio)
bun run test:helper            # prints "29 Swift tests passed" (fails on 0)
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

Give qrtz Accessibility permission (System Settings → Privacy & Security → Accessibility): **qrtz-app** for a `bun start` dev build, **qrtz** for `/Applications/qrtz.app`. Not your terminal: see [dev-permissions.md](dev-permissions.md). Join a test call, turn on captions, and open the participants and chat panels.

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

- [ ] A fresh install opens straight to note (no setup screen, ADR 0163) and downloads nothing until you press download in settings → speech or switch echo or nano on. The first recording prompts for System Audio Recording and Microphone; the first fn press prompts for Input Monitoring; marketplace → installed shows each permission with "allow".
- [ ] The log shows `audio: core-audio-tap` and `asr: parakeet-unified-streaming`.
- [ ] `asr` partial and final events arrive within about 2 s of speech, for both the video and your voice.
- [ ] `afplay /tmp/q/r.wav` plays both sides, and `recording.stopped.durationMs` is about 25000.
- [ ] **Fallback check:** deny System Audio Recording. The log should show `audio core-audio-tap failed …` followed by `audio: screencapturekit`.
- [ ] **Echo check:** without headphones, during a call, remote speech should not appear twice in the transcript. The mic is ducked while AX shows someone else as the active speaker.

## 4. Inside the app (1 min)

```sh
cd apps/desktop && bun start   # tauri dev (or `bun run make` for the .app + .dmg)
```

- [ ] With `bun run make`, `Contents/MacOS/qrtz-helper` is in the .app. In either build, Settings → System shows the helper as connected and not mock.
- [ ] Join a Zoom or Meet call. qrtz offers to record, the floating HUD appears (top right, above Zoom), ⌘⇧R stops recording, and a summary appears.
- [ ] The summary's source reads "on-device · apple intelligence" (ADR 0075; Apple Intelligence on in System Settings › Apple Intelligence & Siri). With Apple Intelligence off, the meeting shows "summary unavailable: apple intelligence is turned off…" with retry, and the transcript-based summary.
- [ ] Helper only: `printf '{"type":"summarize.check","id":"c1"}\n' | .build/debug/qrtz-helper` (from `native/qrtz-helper`) answers `summarize.status` with `"available":true` and `"contextTokens":4096`.

## nano calendar (ADR 0217, 5 min)

1. Fresh install with nano: onboarding lists "calendar (optional)"; nothing prompts until you click allow. Allow: the macOS Calendars prompt names qrtz.
2. Skip onboarding instead, open nano: the upcoming block shows "connect calendar"; click it, grant, and the next 14 days of events appear (titles as in Calendar.app).
3. nano settings → calendar: untick a calendar, its events leave the block; turn calendar off, the block goes back to "connect calendar" and the cache is wiped.
4. Add an event in Calendar.app starting in 2 min: within seconds (EKEventStoreChanged) it shows; at its start the meeting prompt appears (no auto-record unless auto-record is on). Record: the note is titled after the event.
5. Revoke in System Settings › Privacy › Calendars: the block says access is off; nothing crashes.

## Known gaps

- **No real echo cancellation:** the echo gate is a heuristic. The upgrade path is FluidAudio LocalVQE AEC with the system tap as the far-end reference.
- **Background Chrome tabs:** a Meet tab is only seen when it is the active tab in its window, because AX exposes only the active tab's web area.
- **Captions:** captions need to be switched on in Zoom or Meet for `ax.caption`. The transcript itself comes from Parakeet and does not depend on them.

## qrtz talk (dictation, 10 min)

- [ ] `mac/Talk.swift` compiles (Linux can't type-check it). Two Parakeet instances are loaded (meeting + dictation), so check memory.
- [ ] Hold **fn** in TextEdit and speak: the pill shows level and streaming text, and on release the text appears at the caret (`insert.result` method `ax-selected-text`).
- [ ] Chrome/Slack/VS Code (Electron fields often ignore AX writes): text arrives via `paste`, and the clipboard you had before is restored.
- [ ] **Fn/globe (fix for Jared's report):** with "Press 🌐 key to" set to change input source (and again set to show emoji), hold and release **fn** for talk: the input source doesn't change and no emoji picker opens. fn+arrow (Home/End) and fn+F-keys still work. Turn off settings → talk → "fn only for talk": the globe action is back. With Accessibility denied (Input Monitoring only), talk still works (listen-only fallback) and the globe action runs.
- [ ] Input Monitoring / Accessibility denied: a `dictation.hotkey` error is logged and ⌘⇧Space still works.
- [ ] Warm mic on: the first word isn't clipped (pre-roll), and the mic indicator stays on as expected.
- [ ] With an ElevenLabs key: the history meta shows the engine. Pull the network mid-dictation and it shows "on-device (fallback)".
- [ ] Record `talk latency` log lines (firstPartialMs, finalMs, insertedMs) for each engine, and run `bun scripts/bench-talk-asr.mjs speech16k.wav 5`. Put the numbers in docs/research-talk.md §2.

## Helper mux + TCC revocation (H5/H6)

- [ ] Record a meeting, then hold the push-to-talk key: dictation works, the meeting WAV/transcript is uninterrupted, and only one input device is open (Audio MIDI Setup / `log stream` shows a single AVAudioEngine input).
- [ ] Turn Microphone off in System Settings mid-dictation: the pill shows "microphone access was turned off…", the text heard so far is inserted, and the helper logs `permission.revoked` within ~2 s.
- [ ] Turn Screen & System Audio Recording off mid-meeting: a `mic only` badge appears live and the recording continues. Then turn Microphone off: `no audio` appears and the recording still stops cleanly.
- [ ] Check whether macOS kills the helper on revocation (some TCC changes SIGKILL the process). If so, the reconnect path takes over (`transcript-gap`).

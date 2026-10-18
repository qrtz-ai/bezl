# Echo dictation: path, latency budget, tuning plan

Status: phase 1 (measure). Nothing in this document changes runtime behaviour; the bench and the timing log
(ADR 0099) are how each change below gets measured before it ships. Refs are `file:line` at the commit that adds
this doc.

## 1. The path, key down to text

| # | Stage | Where | Tunables (current value) |
|---|-------|-------|--------------------------|
| 1 | Hotkey down → `talk` service sends `dictation.start` | `crates/qrtz-talk/src/service.rs`, `mac/Talk.swift` (`MacHotkeys`) | `talk.*` config, `crates/qrtz-talk/src/config.rs:34-41`: `warmMic` **false**, `preRollMs` **300**, `autoCleanup` **true**, `restoreClipboard` **true**, `showPill` **true**, `echoGate` **true** |
| 2 | Helper picks a recogniser | `QrtzHelperCore/Dictation.swift:334-345`; list from `mac/Platform.swift:56` | first ready of `[NemotronTranscriber, SpeechAnalyzerTranscriber (macOS 26)]`; Apple is only a fallback when Nemotron is busy/fails |
| 3 | Pre-roll drained into the session | `Dictation.swift:186`, `:353-360` | ring of **300 ms**; only has audio when the mic was already running (`warmMic` on, or a quick re-press) |
| 4 | Mic start (cold unless `warmMic`) | `Dictation.swift:289` → `mac/Audio.swift:78` | AVAudioEngine input tap, `bufferSize` **4096** frames (85 ms at 48 kHz) |
| 5 | Downmix + resample to 16 kHz | `mac/Audio.swift:17-40` (`MonoConverter`) → `QrtzHelperCore/Audio.swift:71` (`DSP.Resampler`) | linear interpolation, **no anti-alias low-pass** (48 kHz → 16 kHz) |
| 6 | Echo gate (speakers playing only) | `QrtzHelperCore/DictationEcho.swift`; tap `mac/Audio.swift:242`; env `QRTZ_ECHO_GATE` (`mac/Platform.swift:38-39`) | duck **0.0316** (−30 dB), prior coupling **0.15**, max coupling **0.5**, system-active RMS **0.01**, reference max lag **250 ms** |
| 7 | Feed → recogniser | `Dictation.swift:382-416` (`feed`) → `mac/Transcribers.swift:50` (`appendAudio` + `processBufferedAudio`) | one append per mic block (~1365 samples); Nemotron decodes only per full chunk |
| 8 | Model | `mac/Transcribers.swift:23-48` | FluidAudio 0.17.7 `StreamingNemotronAsrManager(requestedChunkSize: .ms560)`: 8960-sample chunks, int8 encoder on `.cpuAndNeuralEngine`, fused decoder_joint, Swift mel, greedy RNN-T, att_context [56,0] (no lookahead) |
| 9 | Partials (pill) | `Dictation.swift:449-457` (`poll`) | polled every **100 ms** (`:363`); new words only every 560 ms chunk |
| 10 | Key up → `dictation.stop`: tail | `Dictation.swift:460-468` (`stop`) | `DictationTail.tailMs` **50** |
| 11 | Drain: the block being filled at release, then the queue barrier | `Dictation.swift:531-560` (`drainMic`, `nextBlock`) | `nextBlockWaitMs` **120**, `nextBlockSlackMs` **40**, `drainTimeoutMs` **300** (+2× queued) |
| 12 | Close (in-flight append returns) | `Dictation.swift:422` (`close`), deadline `:480-486` | inside one deadline: `finalizeTimeoutMs` **1500** + audio/10 + queued, `maxWaitMs` **20000**, `backlogCapMs` **20000** |
| 13 | Pad + finish | `Dictation.swift:574-597` (`finalText`) | `padMs` **640** of zeros, then `finish()` (FluidAudio pads the remainder to one more chunk) |
| 14 | `dictation.stopped` → Rust cleanup | `crates/qrtz-talk/src/cleanup.rs:59-106` | rules: new paragraph/line, fillers, repeats, spacing, sentence caps, lone "i", dictionary, **always appends "."** |
| 15 | Insert | `mac/Talk.swift` (`MacInsertion`), `Dictation.swift:70` (`TextInserter`) | AX insert; paste fallback with `restoreDelayMs` **250** *before* `insert.result` returns |
| 16 | Host timeouts | `crates/qrtz-talk/src/service.rs:127-133` | started 3000, stopped 3000 + 100/audio s (max 20000), insert 2500, final_ms 1500; mirrors `DictationTail` (`tests/hardening27.rs` pins the Swift defaults) |

Model files: `crates/qrtz-app/src/local_models.json` set `asr` = `nemotron-560ms`, verified into
`~/Library/Application Support/ai.qrtz.desktop/models/nemotron-560ms/` (`.qrtz-verified`), read through
`QRTZ_MODELS_DIR` (`mac/Models.swift`). The helper never downloads.

## 2. Latency budget, release → text

Estimated for a warm model on an M-series Mac; the timing log (§5) replaces the estimates with Jared's numbers.

| Step | Typical | Notes |
|------|---------|-------|
| tail | 50 ms | fixed sleep |
| next block | 0-85 ms (+40 slack) | waits for the 85 ms block that was being filled at the release |
| drain barrier | ~0-5 ms | the queue is normally empty (live RTF ≪ 1) |
| close | ~0-15 ms | an append (chunk decode) may be in flight |
| pad 640 ms → chunk decode | ~14 ms | 560 ms chunk at ~40× RTFx on ANE |
| finish → remainder padded to a chunk → decode | ~14 ms | second encoder call |
| **helper total** | **~130-210 ms** | `release_to_stopped_ms` in the log |
| cleanup (rules) | < 1 ms | `cleanup_ms` |
| insert (AX) | ~5-30 ms | paste fallback: text shows at ⌘V, but `insert.result` waits +250 ms restore |

So release-to-text is ~0.15-0.25 s; the floor is the tail + block alignment (~100 ms) plus two chunk decodes. The
bigger perceived costs are elsewhere: the **head** (cold mic, no pre-roll), the **560 ms partial cadence** (the pill
lags speech by up to 0.6 s + 100 ms poll), and paste-fallback restore time.

## 3. Accuracy levers

| Lever | Today | What moves it |
|-------|-------|---------------|
| Model / chunk tier | Nemotron 560 ms | FluidAudio (M5 Pro, LibriSpeech test-clean full): 560 = 2.71 %, 1120 = 2.58 %, 2240 = 2.64 % WER. 560 also **loses punctuation on long dictations** (FluidAudio #687); 1120 keeps it. |
| Two-pass final | none | Parakeet Unified batch final on release: 1.68 % WER (test-clean, FluidAudio), ~600 MB more disk + RAM, one batch encode on release (~30-80 ms for a 10 s dictation at ~120× RTFx). Partials stay Nemotron. |
| Vocabulary biasing | after the fact (`apply_dictionary` text replace) | Decode-time biasing exists only in FluidAudio's multilingual Nemotron (`setCustomVocabulary`, local-path-only conversion, no published weights) and the Unified/TDT CTC boosting path. Not available on the shipped English Nemotron. |
| Last word (tail) | 50 ms tail + 640 ms pad + finish pad | bench variants `no-tail`, `pad-0`, `pad-1120`, `tail-150` (tail-word loss is scored separately) |
| First word (head) | cold mic, pre-roll only when warm | `warmMic`, `preRollMs`; bench variants `cold-mic-150/300`, `warm-preroll-300` (head-word loss) |
| VAD | none in dictation (push-to-talk) | not a lever for hold-to-talk; matters only if hands-free mode arrives |
| Gain / level | none (Nemotron mel is not normalized) | bench `quiet-0.1` vs `quiet-0.1-normalized` shows whether a peak normalizer helps quiet mics |
| Resampling | linear, no low-pass | aliasing above 8 kHz folds into the band; swap to `AVAudioConverter` (mastering quality) and measure |
| Echo gate | on when speakers play | bench `echo-raw` vs `echo-gated` with a speaker clip |
| Rules cleanup | always on, always adds "." | `echo_score` reports raw vs cleaned WER and formatted WER; the "." on one-word replies and the repeats rule ("that that") are the visible risks |

## 4. Ranked tuning plan

Gain is the expected user-visible effect; risk is what could regress. Every item is measured with the bench
(§5) before and after, on Jared's clips, then confirmed in real use through the timing log.

| Rank | Change | Expected gain | Risk | Measure |
|------|--------|---------------|------|---------|
| 1 | **Head: warm the mic while the hotkey is armed (or default `warmMic` on with the 300 ms pre-roll)**, at least for Bluetooth inputs | Stops first-word clipping, the most common dictation complaint; the Linux run (§6) and bench `cold-mic-*` give the size | mic indicator stays lit; battery; privacy perception | bench `cold-mic-150/300` vs `warm-preroll-300`: headWordsLost; log `preroll_ms` |
| 2 | **Two-pass final: Nemotron partials + Parakeet Unified batch final** (engine `nemotron-560+unified` in the bench) | ~35-40 % fewer word errors on the inserted text (2.71 → 1.68 % on test-clean), full punctuation | +~600 MB model set (signed path), +30-80 ms release latency, two models resident | bench `--engines nemotron-560,nemotron-560+unified`: wer, fwer, finalDecode p90 |
| 3 | **Release path: drop the 640 ms pad (finish already pads one chunk) and trim `tailMs`** | −15-30 ms release-to-text per dictation; one encoder call instead of two | last-word loss if the model needs right context | bench `pad-0`, `no-tail`, `tail-150`: tailWordsLost + release p50/p90; Linux §6 trim/pad rows |
| 4 | **Cleanup: no "." on short (≤ 3 word) or chat-target dictations; keep the repeats rule off digits/intentional doubles** | fewer manual fixes on short replies and search boxes | style change users notice | `echo_score` cleaned vs raw formatted WER and exactAfterCleanup on the short clips (03-yes, chat prompts) |
| 5 | **Resampler: AVAudioConverter (or a windowed-sinc) instead of linear** | small WER gain on sibilants and noisy rooms; removes aliasing | CPU on the audio thread (tiny), behaviour change in Bobs's capture code | bench on 48 kHz recordings of Jared's voice before/after; noise-15db variant |
| 6 | Nemotron 1120 ms tier for dictations longer than ~10 s | −5 % relative WER, punctuation kept on long dictations | partials every 1.1 s (pill feels slower); separate model set | bench engine `nemotron-1120` (`--extra`): wer, fwer, firstPartial |
| 7 | Input peak normalization for quiet mics | robustness on quiet/far mics | clipping or noise boost | `quiet-0.1` vs `quiet-0.1-normalized` |
| 8 | Decode-time vocabulary biasing | names and jargon right the first time | needs a different model (multilingual Nemotron / CTC boosting) + a signed conversion | later; measure on a names clip set |

Not on the list: moving the encoder to the GPU (FluidAudio: int8 on GPU is ~10× slower and can abort in MPSGraph;
`.cpuAndNeuralEngine` is right), and lower chunk tiers (160/80 ms) for dictation (more compute and worse
punctuation for partials nobody reads closely).

## 5. Measuring

### On the Mac (Jared)

```bash
cd ~/code/qrtz && git pull
bash native/qrtz-helper/scripts/echo-bench.sh            # ~25-35 min: shipped model, all variants + engines
bash native/qrtz-helper/scripts/echo-bench.sh --quick    # ~8 min smoke
bash native/qrtz-helper/scripts/echo-bench.sh --extra    # + nemotron-1120/2240 and the Unified two-pass (~1.9 GB, sha256-pinned)
bash native/qrtz-helper/scripts/echo-bench.sh --clips ~/qrtz-echo-bench/my-clips   # your own voice
```

The script builds a release helper from `origin/main` in a scratch worktree (`/tmp/qrtz-echo-bench-wt`), copies
the app's already-verified Nemotron set (falls back to the pinned download from `local_models.json`), synthesizes
the 18 `say` prompts (`scripts/echo-clips.tsv`), runs the sweep and the engine comparison, and scores both with
the real Rust cleanup. Results land in `~/qrtz-echo-bench/results/`:

- `sweep-<stamp>.json`, `engines-<stamp>.json`: raw per-clip runs (hypothesis, reference, WER, tail/head word loss,
  release-to-stopped, first partial, every stage in ms) and a summary per engine × variant (p50/p90/max).
- `*.scored.json`: the same after `cleanup_dictation` (raw vs cleaned, formatted WER).
- `summary-<stamp>.txt`: the table to paste back.

Directly: `qrtz-helper --echo-bench <clips> [--engines a,b] [--variants all|shipped,pad-0] [--speed 1|4]
[--speaker file] [--out file.json]`, then `cargo run -p qrtz-talk --example echo_score -- <report.json>
[--dictionary dict.json]`. `--speed 1` (default) feeds audio in real time; higher speeds are for smoke runs only
(timings are then not real).

### Recording your own clips

1. Make a folder, e.g. `~/qrtz-echo-bench/my-clips/`.
2. Record each phrase with QuickTime (File → New Audio Recording) or Voice Memos, with the mic you dictate with
   (do one set on AirPods if you use them). Leave ~0.5 s of silence before and after.
3. Save as `NN-name.m4a` (or wav/aiff/caf/mp3/flac) and put exactly what you said in `NN-name.txt` next to it,
   written the way you want it typed ("Send it to Sarah at 3pm.").
4. Aim for 15-20 clips: 3-4 one-to-three word replies ("Yes.", "Sounds good."), 5 normal sentences, 3 long
   (20-40 s) ones, names and product words you use (qrtz, people, companies), numbers/dates/times, and one or two
   where the last word is short ("…by Friday.").
5. Optional: `speaker/` subfolder with a clip of the kind of audio you play while dictating (podcast/music), for
   the echo variants.
6. Run `echo-bench.sh --clips <folder>`; add `--dictionary` with your qrtz dictionary JSON to score the
   post-recognition replacements.

### In real use: the timing log

Every dictation writes one content-free line to `~/Library/Logs/qrtz/qrtz.log` (ADR 0083 rules, ADR 0099):

```
[helper] echo timing: asr=nemotron-560ms audio_ms=4210 preroll_ms=0 first_partial_ms=890 appends=49 append_total_ms=181.2 append_max_ms=17.4 live_rtf=0.043 tail_ms=51 drain_ms=118 close_ms=118 finalize_ms=146 release_to_stopped_ms=147 queued_ms=0 word_count=11
talk latency engine=... audio_ms=4210 first_partial_ms=... final_ms=... cleanup_ms=... inserted_ms=... insert=AxSelectedText cleanup=Rules raw_chars=58 inserted_chars=59
```

No transcript, text, or dictionary content: durations, counts, and the recogniser's name only. Flags
`drain_timed_out`, `final_timed_out`, `append_held`, `gate_ducked=x/y` appear only when they happen.
`grep "echo timing" ~/Library/Logs/qrtz/qrtz.log` after a day of normal use is the most useful thing to send back.

## 6. Linux cross-check (same model family, ONNX)

Running; results land in a follow-up commit.

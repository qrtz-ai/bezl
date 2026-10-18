# Echo dictation: path, latency budget, tuning plan

Status: phase 1 (measure). Nothing in this document changes runtime behaviour; the bench and the timing log
(ADR 0099) are how each change below gets measured before it ships. Refs are `file:line` at the commit that adds
this doc.
Cycle 32: §6 has the Linux numbers and §4 is re-ranked from them (ADR 0102); plan item 4, the cleanup, is done
(ADR 0101).

## 1. The path, key down to text

| #   | Stage                                                            | Where                                                                                                              | Tunables (current value)                                                                                                                                                                                                |
| --- | ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | Hotkey down → `talk` service sends `dictation.start`             | `crates/qrtz-talk/src/service.rs`, `mac/Talk.swift` (`MacHotkeys`)                                                 | `talk.*` config, `crates/qrtz-talk/src/config.rs:34-41`: `warmMic` **false**, `preRollMs` **300**, `autoCleanup` **true**, `restoreClipboard` **true**, `showPill` **true**, `echoGate` **true**                        |
| 2   | Helper picks a recogniser                                        | `QrtzHelperCore/Dictation.swift:334-345`; list from `mac/Platform.swift:56`                                        | first ready of `[NemotronTranscriber, SpeechAnalyzerTranscriber (macOS 26)]`; Apple is only a fallback when Nemotron is busy/fails                                                                                      |
| 3   | Pre-roll drained into the session                                | `Dictation.swift:186`, `:353-360`                                                                                  | ring of **300 ms**; only has audio when the mic was already running (`warmMic` on, or a quick re-press)                                                                                                                 |
| 4   | Mic start (cold unless `warmMic`)                                | `Dictation.swift:289` → `mac/Audio.swift:78`                                                                       | AVAudioEngine input tap, `bufferSize` **4096** frames (85 ms at 48 kHz)                                                                                                                                                 |
| 5   | Downmix + resample to 16 kHz                                     | `mac/Audio.swift:17-40` (`MonoConverter`) → `QrtzHelperCore/Audio.swift:71` (`DSP.Resampler`)                      | linear interpolation, **no anti-alias low-pass** (48 kHz → 16 kHz)                                                                                                                                                      |
| 6   | Echo gate (speakers playing only)                                | `QrtzHelperCore/DictationEcho.swift`; tap `mac/Audio.swift:242`; env `QRTZ_ECHO_GATE` (`mac/Platform.swift:38-39`) | duck **0.0316** (−30 dB), prior coupling **0.15**, max coupling **0.5**, system-active RMS **0.01**, reference max lag **250 ms**                                                                                       |
| 7   | Feed → recogniser                                                | `Dictation.swift:382-416` (`feed`) → `mac/Transcribers.swift:50` (`appendAudio` + `processBufferedAudio`)          | one append per mic block (~1365 samples); Nemotron decodes only per full chunk                                                                                                                                          |
| 8   | Model                                                            | `mac/Transcribers.swift:23-48`                                                                                     | FluidAudio 0.17.7 `StreamingNemotronAsrManager(requestedChunkSize: .ms560)`: 8960-sample chunks, int8 encoder on `.cpuAndNeuralEngine`, fused decoder_joint, Swift mel, greedy RNN-T, att_context [56,0] (no lookahead) |
| 9   | Partials (pill)                                                  | `Dictation.swift:449-457` (`poll`)                                                                                 | polled every **100 ms** (`:363`); new words only every 560 ms chunk                                                                                                                                                     |
| 10  | Key up → `dictation.stop`: tail                                  | `Dictation.swift:460-468` (`stop`)                                                                                 | `DictationTail.tailMs` **50**                                                                                                                                                                                           |
| 11  | Drain: the block being filled at release, then the queue barrier | `Dictation.swift:531-560` (`drainMic`, `nextBlock`)                                                                | `nextBlockWaitMs` **120**, `nextBlockSlackMs` **40**, `drainTimeoutMs` **300** (+2× queued)                                                                                                                             |
| 12  | Close (in-flight append returns)                                 | `Dictation.swift:422` (`close`), deadline `:480-486`                                                               | inside one deadline: `finalizeTimeoutMs` **1500** + audio/10 + queued, `maxWaitMs` **20000**, `backlogCapMs` **20000**                                                                                                  |
| 13  | Pad + finish                                                     | `Dictation.swift:574-597` (`finalText`)                                                                            | `padMs` **640** of zeros, then `finish()` (FluidAudio pads the remainder to one more chunk)                                                                                                                             |
| 14  | `dictation.stopped` → Rust cleanup                               | `crates/qrtz-talk/src/cleanup.rs:59-106`                                                                           | rules: new paragraph/line, fillers, repeats of stutter-prone words only, spacing (no space inside dotted tokens), sentence caps, lone "i", dictionary, **"." only past 3 words** (ADR 0101)                             |
| 15  | Insert                                                           | `mac/Talk.swift` (`MacInsertion`), `Dictation.swift:70` (`TextInserter`)                                           | AX insert; paste fallback with `restoreDelayMs` **250** _before_ `insert.result` returns                                                                                                                                |
| 16  | Host timeouts                                                    | `crates/qrtz-talk/src/service.rs:127-133`                                                                          | started 3000, stopped 3000 + 100/audio s (max 20000), insert 2500, final_ms 1500; mirrors `DictationTail` (`tests/hardening27.rs` pins the Swift defaults)                                                              |

Model files: `crates/qrtz-app/src/local_models.json` set `asr` = `nemotron-560ms`, verified into
`~/Library/Application Support/ai.qrtz.desktop/models/nemotron-560ms/` (`.qrtz-verified`), read through
`QRTZ_MODELS_DIR` (`mac/Models.swift`). The helper never downloads.

## 2. Latency budget, release → text

Estimated for a warm model on an M-series Mac; the timing log (§5) replaces the estimates with Jared's numbers.

| Step                                          | Typical             | Notes                                                                       |
| --------------------------------------------- | ------------------- | --------------------------------------------------------------------------- |
| tail                                          | 50 ms               | fixed sleep                                                                 |
| next block                                    | 0-85 ms (+40 slack) | waits for the 85 ms block that was being filled at the release              |
| drain barrier                                 | ~0-5 ms             | the queue is normally empty (live RTF ≪ 1)                                  |
| close                                         | ~0-15 ms            | an append (chunk decode) may be in flight                                   |
| pad 640 ms → chunk decode                     | ~14 ms              | 560 ms chunk at ~40× RTFx on ANE                                            |
| finish → remainder padded to a chunk → decode | ~14 ms              | second encoder call                                                         |
| **helper total**                              | **~130-210 ms**     | `release_to_stopped_ms` in the log                                          |
| cleanup (rules)                               | < 1 ms              | `cleanup_ms`                                                                |
| insert (AX)                                   | ~5-30 ms            | paste fallback: text shows at ⌘V, but `insert.result` waits +250 ms restore |

So release-to-text is ~0.15-0.25 s; the floor is the tail + block alignment (~100 ms) plus two chunk decodes. The
bigger perceived costs are elsewhere: the **head** (cold mic, no pre-roll), the **560 ms partial cadence** (the pill
lags speech by up to 0.6 s + 100 ms poll), and paste-fallback restore time.

## 3. Accuracy levers

| Lever              | Today                                            | What moves it                                                                                                                                                                                                                                 |
| ------------------ | ------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Model / chunk tier | Nemotron 560 ms                                  | FluidAudio (M5 Pro, LibriSpeech test-clean full): 560 = 2.71 %, 1120 = 2.58 %, 2240 = 2.64 % WER. 560 also **loses punctuation on long dictations** (FluidAudio #687); 1120 keeps it.                                                         |
| Two-pass final     | none                                             | Parakeet Unified batch final on release: 1.68 % WER (test-clean, FluidAudio), ~600 MB more disk + RAM, one batch encode on release (~30-80 ms for a 10 s dictation at ~120× RTFx). Partials stay Nemotron.                                    |
| Vocabulary biasing | after the fact (`apply_dictionary` text replace) | Decode-time biasing exists only in FluidAudio's multilingual Nemotron (`setCustomVocabulary`, local-path-only conversion, no published weights) and the Unified/TDT CTC boosting path. Not available on the shipped English Nemotron.         |
| Last word (tail)   | 50 ms tail + 640 ms pad + finish pad             | bench variants `no-tail`, `pad-0`, `pad-1120`, `tail-150` (tail-word loss is scored separately)                                                                                                                                               |
| First word (head)  | cold mic, pre-roll only when warm                | `warmMic`, `preRollMs`; bench variants `cold-mic-150/300`, `warm-preroll-300` (head-word loss)                                                                                                                                                |
| VAD                | none in dictation (push-to-talk)                 | not a lever for hold-to-talk; matters only if hands-free mode arrives                                                                                                                                                                         |
| Gain / level       | none (Nemotron mel is not normalized)            | Linux §9: a causal AGC recovers a quiet mic fully but costs +9 % on a normal mic, so only a quiet-mic-only gain is a candidate                                                                                                                |
| Resampling         | linear, no low-pass                              | Linux §9: band-limited is -16 % / -26 % errors in noise, equal on clean; swap to `AVAudioConverter` (mastering quality)                                                                                                                       |
| Echo gate          | on when speakers play                            | bench `echo-raw` vs `echo-gated` with a speaker clip                                                                                                                                                                                          |
| Rules cleanup      | always on; "." only past 3 words (ADR 0101)      | `echo_score` reports raw vs cleaned WER and formatted WER. Cycle 32 (ADR 0101) fixed the repeats rule (digits, doubles, contractions, `see plus plus`) and the dot spacing (`qrtz.ai`, `p.m.`); `tests/cleanup_accuracy.rs` holds the vectors |

## 4. Ranked tuning plan

Gain is the expected user-visible effect; risk is what could regress. Every item is measured with the bench
(§5) before and after, on Jared's clips, then confirmed in real use through the timing log.

| Rank | Change                                                                                                                                                                                        | Expected gain                                                                                                                                                                                                                                                                                                                                                                      | Risk                                                                                                                                                                                                                                                        | Measure                                                                                                                                             |
| ---- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | **Two-pass final: Nemotron partials + a batch Parakeet final on release** (bench engine `nemotron-560+unified`)                                                                               | Measured on Linux (§6): Parakeet TDT v2 batch 3.05 % vs Nemotron 560 4.06 % WER (−25 % errors; test-clean 2.84 → 2.16 %). On a quick release, the realistic case, it is 2.70 % vs 4.33 % (−38 %), and last-word misses drop from 12.0 % to 4.7 %: the batch pass also fixes the cut-off last word. FluidAudio's Unified batch on ANE: 2.71 → 1.68 % (−38 %), with full punctuation | +~600 MB model set (signed path); one batch encode on release (~60 ms for a 7 s dictation at ~120× RTFx on ANE; 440 ms p50 on the box CPU); two models resident                                                                                             | bench `--engines nemotron-560,nemotron-560+unified`: wer, fwer, finalDecode p90                                                                     |
| 2    | **Protect the last word: pad 1120 ms instead of 640 (keep the pad; "drop the pad" is refuted).** Proposed to Bobs (review §12.31 a)                                                           | Measured (§6), quick release: pad 1120 halves last-word misses, 12.0 → 6.3 %, and brings WER from 4.33 % back to 4.07 %, the level of a slow release (4.06 %). A 150 ms tail instead gets 12.0 → 9.0 % (WER 4.21 %). Dropping the pad or relying on the finish padding alone: 37-67 % misses                                                                                       | one more 560 ms chunk decoded on release (~14 ms on ANE, to be measured); talk's `HelperTail.pad_ms` must follow (`tests/hardening27.rs` pins it to the Swift default)                                                                                      | bench sweep `shipped` vs `pad-1120` (and `tail-150`, `pad-0`): tailWordsLost + releaseToStopped p50/p90 on Jared's clips                            |
| 3    | **Resampler: band-limited (AVAudioConverter, or a windowed sinc) instead of linear** (was rank 5; cycle 37, ADR 0115). Proposed to Bobs (review §12.34)                                       | Measured (§9): no change on clean speech; in noise -16 % errors (white, 15 dB) and -26 % (8-20 kHz, 15 dB), both resolved; with it, 8-20 kHz noise costs nothing measurable                                                                                                                                                                                                        | tiny CPU cost on the audio thread; the converter holds a few ms of audio that the release drain must flush; a behaviour change in Bobs's capture code                                                                                                       | bench `--resample` linear vs converter on 48 kHz recordings, clean and noise-15db: wer; releaseToStopped p90                                        |
| 4    | **Head: warm the mic while the hotkey is armed (or `warmMic` on by default with the 300 ms pre-roll)**, at least for Bluetooth inputs                                                         | Stops first-word clipping. Not measured yet (LibriSpeech clips start with silence); bench `cold-mic-*` gives the size on Jared's Mac                                                                                                                                                                                                                                               | mic indicator stays lit; battery; privacy perception                                                                                                                                                                                                        | bench `cold-mic-150/300` vs `warm-preroll-300`: headWordsLost; log `preroll_ms`                                                                     |
| 5    | ~~Cleanup: no "." on short (≤ 3 word) dictations; keep the repeats rule off digits and intentional doubles~~ **Done** in cycle 32 (`cda052c`, ADR 0101), with the dot-spacing fix (N159-N161) | fewer manual fixes on short replies, search boxes, PINs, domains and file names; 44 hand-written vectors                                                                                                                                                                                                                                                                           | a style change users will notice                                                                                                                                                                                                                            | `echo_score` cleaned vs raw formatted WER and exactAfterCleanup on the short clips (03-yes, chat prompts)                                           |
| 6    | Nemotron 1120 ms tier for long dictations                                                                                                                                                     | Punctuation kept on long dictations (FluidAudio #687). Not a WER lever: with a pad that covers its chunk (1280 ms) it scored 3.93 % vs 4.02 % for 560 ms at pad 1120 on Linux (FluidAudio 2.71 → 2.58 %). The 4.35 % at pad 640 was the pad being shorter than one 1120 chunk                                                                                                      | partials every 1.1 s, so the pill feels slower; a separate model set                                                                                                                                                                                        | bench engine `nemotron-1120` (`--extra`): wer, fwer, firstPartial                                                                                   |
| 7    | Input normalization for quiet mics, **quiet-mic only** (§9: a mic 20 dB down costs +23 % errors and a causal AGC recovers it all, but the same AGC on a normal mic costs +9 %; ADR 0115)      | robustness on quiet or far mics                                                                                                                                                                                                                                                                                                                                                    | an always-on normaliser hurts normal mics (measured); needs a gated design (gain 1 until the speech level is known to be low) and its own Linux rows first                                                                                                  | `talk latency` `level_p90_dbfs` (§10) decides whether it is needed at all; then `quiet-0.1` vs `quiet-0.1-normalized` and a normal-level clip, both |
| 8    | Decode-time vocabulary biasing                                                                                                                                                                | names and jargon right the first time                                                                                                                                                                                                                                                                                                                                              | needs a different model. Nemotron 3.5 multilingual scored 6.59 % on English (§6, auto language) and 5.72 % with English forced (§9, still +41 % errors vs the shipped model), so CTC boosting (Unified/TDT) is the likelier route, plus a signed conversion | later; measure on a names clip set                                                                                                                  |

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
talk latency engine=... audio_ms=4210 head_ms=0 first_partial_ms=... final_ms=... cleanup_ms=... inserted_ms=... insert=AxSelectedText cleanup=Rules raw_chars=58 inserted_chars=59 level_blocks=49 level_p10_dbfs=-61 level_p90_dbfs=-24
```

With Parakeet (echo's recogniser since 2026-10-18), `asr=parakeet-tdt-v2` and the line ends with
`segments=3 bg_segments=2 cap_cuts=0 tail_decode_ms=84.2 backlog_wait_ms=0.0`. Those are: segments decoded (tail
included), the ones decoded in the background while the key was held, cuts made by the 15 s cap, the release's tail
decode (`-1`: the tail had no speech), and how long the release waited for background decodes still running.
`decode_failures=n` appears only when a decode threw.

Segment rule (Linux eval, 2026-10-18): the exact `Segmented.swift` over Parakeet TDT 0.6B v2 (sherpa-onnx int8, so
WER is indicative, not the Core ML number) on 40 long LibriSpeech dictations (p50 43 s). Defaults: pause 300 ms,
cap 15 s (pad included), minimum segment 2 s, no overlap, VAD floor 0.0002 RMS.

| run                                    | WER    | segments | tail audio at release p50 / p90 / max |
| -------------------------------------- | ------ | -------- | ------------------------------------- |
| whole clip                             | 2.83 % | 1        | 43.2 / 60.3 / 65.7 s                  |
| segmented, 480 ms overlap on every cut | 6.89 % |          |                                       |
| segmented, overlap on cap cuts only    | 3.26 % |          |                                       |
| segmented, defaults                    | 2.55 % | 8.2      | 5.3 / 10.7 / 14.0 s                   |
| quiet (×0.1), whole clip               | 3.13 % | 1        |                                       |
| quiet, VAD floor 0.001, no minimum     | 3.91 % | 12.9     |                                       |
| quiet, defaults                        | 3.04 % | 8.8      | 5.0 / 10.7 / 14.0 s                   |

Overlap hurts because Parakeet often drops a segment that starts inside a word; the cuts already land in silence.
`QRTZ_ECHO_SEGMENT_*` overrides each knob, and `scripts/echo-bench.sh` compares `parakeet-segmented`,
`parakeet-whole` and `nemotron-560` on the Mac (latency, WER, footprint), which this eval doesn't measure.

`tail_ms`, `drain_ms`, `close_ms` and `finalize_ms` are cumulative: each is the time from the release to the end of that
stage, so `finalize_ms − close_ms` is the pad + finish decode. No transcript, text, or dictionary content: durations,
counts, and the recogniser's name only. Flags
`drain_timed_out`, `final_timed_out`, `append_held`, `gate_ducked=x/y` appear only when they happen. `armed=1` (ADR 0104)
means the press opened the mic; talk's `head_ms` (ADR 0107) is the audio the helper heard before talk's start (a
press queued behind a finishing dictation shows seconds here). `level_p10_dbfs` / `level_p90_dbfs` (ADR 0112) are the
mic's floor between words and about its speech level, in dBFS, after the echo gate; §10 says how to read them.
`grep "echo timing" ~/Library/Logs/qrtz/qrtz.log` after a day of normal use is the most useful thing to send back.

## 6. Linux cross-check (same model family, ONNX)

Same model family through sherpa-onnx 1.13.8 (int8 ONNX exports of the same NVIDIA checkpoints) on the box CPU
(4 threads, shared 8 vCPU). The set is 300 seeded LibriSpeech utterances (150 test-clean + 150 test-other, 35 min).
Audio is fed in 85 ms blocks like the Mac tap, then padded, then `input_finished`. "Last word missed" means the
reference's last word isn't among the hypothesis's last two words. The final decode is the time from release to
the final text. "trim" cuts the clip to about 40 ms after the last speech frame, like releasing the key right after
the last word.

| Run                                                                    | test-clean WER | test-other WER | all WER | last word missed | CPU RTF | final decode p50 / p90 |
| ---------------------------------------------------------------------- | -------------: | -------------: | ------: | ---------------: | ------: | ---------------------: |
| `nem560-pad640` (shipped-like)                                         |         2.84 % |         5.46 % |  4.06 % |            6.7 % |   0.290 |           156 / 297 ms |
| `nem560-pad0` (last partial chunk not decoded)                         |         4.42 % |         7.86 % |  6.01 % |           37.0 % |   0.266 |               0 / 0 ms |
| `nem560-trim-pad640` (quick release)                                   |         2.94 % |         5.95 % |  4.33 % |           12.0 % |   0.298 |           166 / 315 ms |
| `nem560-trim-finishpad` (quick release, remainder padded to 8960 only) |         6.36 % |        11.26 % |  8.63 % |           66.7 % |   0.282 |             0 / 164 ms |
| `nem1120-pad640`                                                       |         3.20 % |         5.69 % |  4.35 % |           15.3 % |   0.182 |           169 / 233 ms |
| `nem35-560-pad640` (Nemotron 3.5 multilingual, auto language)          |         4.52 % |         8.98 % |  6.59 % |            7.0 % |   0.274 |           145 / 269 ms |
| `parakeet-v2-offline` (batch, whole utterance)                         |         2.16 % |         4.08 % |  3.05 % |            5.0 % |   0.077 |          440 / 1017 ms |
| `nem560-pad1120` ²                                                     |         2.84 % |         5.39 % |  4.02 % |            5.7 % |   0.523 |          352 / 1045 ms |
| `nem560-trim-pad1120` ² (quick release, pad 1120)                      |         2.87 % |         5.46 % |  4.07 % |            6.3 % |   0.502 |          342 / 1038 ms |
| `nem560-trim150-pad640` ² (release with a 150 ms tail)                 |         2.84 % |         5.80 % |  4.21 % |            9.0 % |   0.462 |           186 / 544 ms |
| `nem1120-pad1280` ² (pad covers one 1120 chunk)                        |         2.87 % |         5.16 % |  3.93 % |            7.3 % |   0.352 |           330 / 747 ms |
| `parakeet-v2-offline-trim` ² (batch, quick release)                    |         1.78 % |         3.78 % |  2.70 % |            4.7 % |   0.094 |          488 / 1145 ms |

² Cycle 32 rows: 2 threads each, four runs at once on a box loaded to ~20 (other jobs), so their RTF and decode times are
not comparable with the rows above (greedy decoding, so WER and last-word numbers are). Same `evalset.json`.

How to read it:

- WER is directly comparable to FluidAudio's numbers. The shipped 560 ms set scores 2.84 % on test-clean here vs
  2.71 % on FluidAudio's full set, so the ONNX path is a fair proxy for accuracy.
- RTF and latency are CPU numbers. On the Mac's Neural Engine everything is 5-10× faster, so only the ordering
  carries over.
- sherpa's `input_finished` does not pad the last partial chunk. FluidAudio's `finish()` does, so the `pad0`,
  `finishpad` and `1120` (pad 640 < 1120 chunk) rows overstate last-word loss for the Mac. What they do show is that
  the last chunk must be pushed through with enough trailing context. The Mac bench `pad-0` variant is the real test
  of dropping the pad.
- Quick releases are the realistic dictation case: the `trim` rows show that trailing context after the last word
  matters even with the shipped pad. That is why plan item 2 is "protect the last word", not "cut the pad".
- The pad is the lever for a quick release (cycle 32): `trim-pad1120` is as good as a slow release with the shipped
  pad (6.3 % vs 6.7 % last word, 4.07 % vs 4.06 % WER). A longer tail recovers less (`trim150`, 9.0 %), so the extra
  right context the decoder needs is cheaper as zeros than as waiting for the user's audio.
- The batch final is not hurt by a quick release (`parakeet-v2-offline-trim` 2.70 %, last word 4.7 %, vs 3.05 % on
  the untrimmed clips; the differences are spread over whole utterances, about noise for 300 clips). It removes the
  accuracy gap and the cut-off last word together, which is why it stays rank 1. On a quick release it sometimes
  ends on "," instead of "."; talk's cleanup already strips a trailing comma and adds the "." (ADR 0101).
- Nemotron 3.5 multilingual was run with automatic language detection. Forcing `en` may close part of the gap, but
  it is not a drop-in improvement for English.

Reproduce it: [`docs/echo/linux-crosscheck/`](linux-crosscheck/README.md) (`run.py`, `evalset.py`, `run-all.sh`
for every row; dev-box only, models and LibriSpeech fetched once, nothing over the network at run time). Field
meanings are in its README. The box copy with per-utterance hypotheses is `/workspace/echo-linux/results/*.json`.

## 7. Cycle 33: tail vs pad, cold head, resampler (Linux, ADR 0104)

Same harness and 300 utterances as §6, three runs at a time with 2 threads on a box loaded to ~15, so only the
ordering of the decode times carries over. The Mac bench (`echo-bench.sh --variants shipped,tail-150,pad-1120,
cold-mic-150,cold-mic-300,warm-preroll-300 --no-engines`) is still the gate for any default change.

**Tail vs pad, emulating FluidAudio** (`--chunkpad 8960`: after the pad, `finish()` pads the remainder to one more
chunk, which §6's rows leave out). "keep" is the audio kept after the last speech frame: 90 ms ≈ a release right at
the word's end plus the 50 ms tail; 190 ms = the 150 ms tail.

| Run (quick release)                 | all WER | last word missed | final decode p50 / p90 (CPU) | release cost on the Mac           |
| ----------------------------------- | ------: | ---------------: | ---------------------------: | --------------------------------- |
| `fa-keep90-pad640` (shipped 50/640) |  4.21 % |            8.7 % |                 373 / 732 ms | today                             |
| `fa-keep190-pad640` (tail 150)      |  4.14 % |            7.7 % |                 343 / 689 ms | +100 ms (the tail is a real wait) |
| `fa-keep90-pad1120` (pad 1120)      |  4.09 % |            6.3 % |                 649 / 977 ms | one more chunk decode, ~14 ms est |

The pad wins on both counts: 28 % fewer last-word misses (8.7 → 6.3 %) for one extra 560 ms chunk through the
encoder, against the tail's 11 % for a fixed +100 ms. The tail is out on latency alone. The pad's cost on the CPU
(+250-275 ms here, loaded) says nothing about the Neural Engine; it ships only when the Mac bench shows the miss rate
down and `releaseToStoppedMs` p90 up by less than 40 ms (review §12.31 (a)).

**Cold head** (`--headdrop`: leading silence cut to 20 ms before the first speech, then this much dropped, like a
mic that opens after the user starts talking):

| Run            | all WER | first word missed |
| -------------- | ------: | ----------------: |
| `head-drop0`   |  4.07 % |             4.3 % |
| `head-drop150` |  4.77 % |            16.7 % |
| `head-drop300` |  6.95 % |            41.7 % |

Every 150 ms of mic start the user talks into costs ~12 points of first-word misses. The press now opens the mic
(ADR 0104), so the host's round trip and engine handshake no longer count; the device's own start still does, and
the `armed=1` timing lines' `preroll_ms` shows how much was saved per dictation.

**Resampler** (`--hf 10`: the clip at 48 kHz with 8-20 kHz noise at 10 dB SNR, a fan, hiss or sibilants above the
band, then back to 16 kHz):

| Run                                               | test-clean | test-other | all WER | last word missed |
| ------------------------------------------------- | ---------: | ---------: | ------: | ---------------: |
| `hf10-linear` (today's `DSP.Resampler`)           |     4.75 % |    11.82 % |  8.02 % |           12.7 % |
| `hf10-poly` (band-limited, like AVAudioConverter) |     2.91 % |     5.46 % |  4.09 % |            7.7 % |

Linear interpolation folds everything above 8 kHz back into the band the model listens to; a band-limited
converter drops it (4.09 % is the clean 4.07 %). The size depends on how much the room puts above 8 kHz, but the
direction is not in doubt. The helper change (AVAudioConverter, mastering quality, stateful per block in
`MonoConverter`) and the bench's `--resample linear|converter` comparison are written and wait for a Mac build.
Landed 2026-10-18 (ADR 0115): AVAudioConverter at `.max` quality is the default; one converter per input rate,
kept across blocks and recreated on a rate or device change; a dictation stop flushes it (end of stream) after
the drain and before the pad (`MicSource.flush`). `QRTZ_ECHO_RESAMPLER=linear` brings back the old path for a
comparison. No input AGC. Still to measure on a Mac: `echo-bench.sh --resample`.
Harness flags (`--keep`, `--headdrop`, `--hf`, `--resampler`, `--chunkpad` after the pad) are in the box copy,
`/workspace/echo-linux/run.py`.

## 8. Cycle 34: the armed head in talk's wait (ADR 0107), harness in the repo

- **Fixed, talk side (N166, `crates/qrtz-talk`):** since ADR 0104 a dictation's audio starts at the press, not at
  talk's start. Talk's stop wait now counts that head (`head_ms`), so a press held through a slow finalize gets
  the helper's full final instead of the last partial. Rig numbers (paused clock, production timeouts): a 0.5 s
  hold behind a 6.5 s finalize used to get 5.54 s and paste the partial; it now gets 22.2 s and the helper's
  final at 6.5 s. Nothing changes when the helper keeps up.
- **Harness:** the §7 flags (`--keep`, `--headdrop`, `--hf`, `--resampler`) and cycle 33's DSP and paired
  comparison (`dsp.py`, `compare.py`, `score.py`) are now in `linux-crosscheck/`, with the §7 rows in `run-all.sh`.
  Re-running `hf10-linear` (first 20 utterances) with the repo copy gave the same hypotheses as §7's run, 20 of 20.
- **Still the gate:** items 1 (two-pass final) and 2 (pad 1120) need the Mac bench before a default changes.

## 9. Cycle 35: skip the module flush wait (ADR 0109); input-side Linux rows

- **Fixed, talk side (N165, `crates/qrtz-talk`):** when `dictation.stopped` text is non-empty it already wins, so
  `finish_inner` no longer waits up to `final_ms` (1.5 s) for a speech-module flush. Still calls `finish` so the
  session exits. Shipped builds (`HelperAsrSpeech`) are unchanged. Rig: a 2 s module flush used to pin
  `talk latency` `final_ms` at 1500; it is now under 500 ms.
- **Input-side Linux cross-check** (harness rows from cycle 33, run this cycle; 300 utterances, 6 threads): 48 kHz
  mic through the helper's linear resampler vs a windowed sinc (clean, white noise 15 dB, 8–20 kHz noise 15 dB);
  quiet mic ×0.1 raw / peak / causal AGC; Nemotron 3.5 with language forced to English. Numbers fill in below as
  the queue finishes; paired bootstrap comparisons land next to the table.

Paired comparisons are `compare.py` (95 % bootstrap interval of the difference over utterances). Rows marked
⁴ ran four at a time with 4 threads on a box loaded to ~17, so only WER and last-word numbers compare.

| Row                                                                  | WER all | Last word missed | vs                        | Difference (95 % interval)                                                                                                 |
| -------------------------------------------------------------------- | ------: | ---------------: | ------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `nem560-pad640` (16 kHz clip, §6)                                    |  4.06 % |            6.7 % |                           |                                                                                                                            |
| `nem560-r48lin-pad640` (48 kHz mic, helper's linear)                 |  4.28 % |            7.7 % | `nem560-pad640`           | +0.23 pts (+6 %), [-0.11, +0.61]: not resolved                                                                             |
| `nem560-r48sinc-pad640` (48 kHz mic, windowed sinc)                  |  4.25 % |            7.0 % | `r48lin`                  | -0.03 pts, [-0.20, +0.13]: no difference                                                                                   |
| `nem560-wn15-pad640` (white noise 15 dB)                             |  5.55 % |            9.3 % | `nem560-pad640`           | +1.49 pts (+37 %), [+0.89, +2.15]                                                                                          |
| `nem560-r48lin-wn15-pad640` ⁴ (48 kHz, linear, wn 15)                |  5.75 % |           10.0 % | `wn15`                    | +0.21 pts, [-0.29, +0.69]: not resolved                                                                                    |
| `nem560-r48lin-hf15-pad640` ⁴ (48 kHz, linear, 8-20 kHz noise 15 dB) |  5.82 % |            9.0 % | `nem560-pad640`           | +1.77 pts (+44 %), [+1.12, +2.49]                                                                                          |
| `nem560-r48sinc-wn15-pad640` ⁴ (48 kHz, sinc, wn 15)                 |  4.85 % |            8.3 % | `r48lin-wn15`             | **-0.90 pts (-16 %), [-1.44, -0.39]**; 53 better / 21 worse                                                                |
| `nem560-r48sinc-hf15-pad640` ⁴ (48 kHz, sinc, 8-20 kHz noise 15 dB)  |  4.28 % |            7.7 % | `r48lin-hf15`             | **-1.54 pts (-26 %), [-2.14, -0.99]**; 62 better / 14 worse. vs clean `nem560-pad640`: +0.23, [-0.09, +0.61], not resolved |
| `nem560-q01-pad640` ⁴ (quiet mic, ×0.1)                              |  4.99 % |            9.0 % | `nem560-pad640`           | +0.94 pts (+23 %), [+0.39, +1.56]                                                                                          |
| `nem560-q01-peak-pad640` ⁴ (quiet, whole-clip peak, oracle)          |  4.11 % |            7.0 % | `q01`                     | -0.88 pts (-18 %), [-1.52, -0.32]                                                                                          |
| `nem560-q01-agc-pad640` ⁴ (quiet, causal peak-hold AGC)              |  4.09 % |            7.0 % | `q01`                     | -0.90 pts (-18 %), [-1.54, -0.33]; vs clean +0.03, [-0.07, +0.15]: the whole loss recovered                                |
| `nem560-agc-pad640` ⁵ (normal level, same AGC)                       |  4.44 % |            8.3 % | `nem560-pad640`           | **+0.38 pts (+9 %), [-0.04, +0.85]**, P(not worse) 0.03; first word missed 4.3 → 6.3 %                                     |
| `nem560-wn15-agc-pad640` ⁵ (wn 15, same AGC)                         |  6.07 % |            9.3 % | `wn15`                    | **+0.52 pts (+9 %), [+0.05, +1.05]**; first word missed 9.7 %                                                              |
| `nem35-560-en-pad640` ⁴ (Nemotron 3.5, language `en`)                |  5.72 % |            6.0 % | `nem35-560-pad640` (auto) | -0.87 pts (-13 %), [-1.85, -0.16]; vs `nem560-pad640` **+1.66 pts (+41 %), [+1.03, +2.33]**                                |

⁵ Cycle 37 rows: two at a time, 4 threads, box loaded to ~20-25. Same harness, seed and `evalset.json`.

What this says (cycle 37, all rows in; ADR 0115):

- **Resampler (now rank 3):** on clean speech the helper's linear resampler costs nothing measurable. In noise it
  is the biggest input-side cost we can remove: it folds everything above 8 kHz back into the band the model
  hears, so a band-limited converter cuts errors by 16 % with broadband (white) noise and 26 % with 8-20 kHz noise,
  both resolved. With the band-limited path, 8-20 kHz noise at 15 dB is indistinguishable from clean speech. A
  fan, laptop fan, air conditioning or keyboard hiss puts energy there; the MacBook mic runs at 48 kHz.
- **Quiet mic (rank 7):** 20 dB down costs +23 %, and a causal peak-hold normaliser recovers all of it. But the
  same normaliser on a mic at normal level costs +9 % (clean, interval just touches 0) and +9 % in noise
  (resolved), with more first words lost (4.3 → 6.3 %, 9.7 % in noise): it boosts the floor before the first
  word by up to 10× and then steps down, and pushes normal speech to a level the model wasn't trained on. **So an
  always-on normaliser is out.** If rank 7 is built, it engages only for a quiet mic (gain 1 until the speech
  level is known to be low, never above the eval set's speech level), and that design needs its own rows first.
  Whether Jared's mics are quiet is still the `talk latency` level line (§10).
- **Noise:** 15 dB of white noise is the biggest cost measured (+37 %); the band-limited resampler removes the
  part above 8 kHz (-16 %), a normaliser makes it worse.
- **Nemotron 3.5 (rank 8):** forcing English helps it (-13 %) but it stays 41 % worse than the shipped English
  Nemotron. Not a route to vocabulary biasing for English.

## 10. Cycle 36: the mic level on the `talk latency` line (ADR 0112)

Talk now logs `level_p10_dbfs` and `level_p90_dbfs` per dictation: the 10th and 90th percentile of the helper's
per-block level (`dictation.level`, block RMS, 1 dB bins), after the echo gate. `levels.py` in
`linux-crosscheck/` runs the same meter over the eval set's clips in each input condition:

| Condition (row)            | `level_p90_dbfs` median (10th–90th pct of clips) | `level_p10_dbfs` median | spread p90 − p10 |    WER |
| -------------------------- | -----------------------------------------------: | ----------------------: | ---------------: | -----: |
| clean (`nem560-pad640`)    |                                  -21 (-26 … -18) |                     -58 |            36 dB | 4.06 % |
| quiet mic ×0.1 (`q01`)     |                                  -41 (-46 … -38) |                     -78 |            36 dB | 4.99 % |
| white noise 15 dB (`wn15`) |                                  -21 (-26 … -18) |                     -40 |            19 dB | 5.55 % |

How to read Jared's lines (`grep "talk latency" ~/Library/Logs/qrtz/qrtz.log`):

- `level_p90_dbfs` of about -30 or louder: near the eval set's level, where 4.06 % was measured; a normaliser has
  little to gain.
- Around -40 or quieter: the quiet-mic regime. If most lines from one mic look like this, rank 7 (a normaliser in
  the helper) is worth building. The `q01-agc` row recovers the whole loss, but the same AGC costs +9 % on a mic
  at normal level (§9), so it must engage only for a quiet mic.
- A spread (`level_p90_dbfs − level_p10_dbfs`) near 20 dB or less: a noisy room or a noisy mic. Level fixes
  don't help; the mic choice or the echo gate does.
- `level_saturated=n`: blocks at or above -12 dBFS (the helper's meter clamp), a hot mic; only a problem if most
  of a dictation is there.
- LibriSpeech is read speech recorded close; real dictation pauses more, so expect a lower p10 than the table.
  Compare mics with each other rather than with the table's exact numbers.

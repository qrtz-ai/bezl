# Echo dictation: path, latency budget, tuning plan

Status: phase 1 (measure). Nothing in this document changes runtime behaviour; the bench and the timing log
(ADR 0099) are how each change below gets measured before it ships. Refs are `file:line` at the commit that adds
this doc.
Cycle 32: §6 has the Linux numbers and §4 is re-ranked from them (ADR 0102); plan item 4, the cleanup, is done
(ADR 0101).
2026-10-18: echo dictates with Parakeet TDT v2, segmented at pauses; nano keeps Nemotron streaming (ADR 0118). §11
has the long-dictation eval, the rule and the helper spec. §1-§10 describe the Nemotron path as it was.

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

| Rank | Change                                                                                                                                                                                                                                                                                                                                                | Expected gain                                                                                                                                                                                                                                                                                                                                                                      | Risk                                                                                                                                                                                                                                                        | Measure                                                                                                                                                                                                                                    |
| ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 1    | **Two-pass final: Nemotron partials + a batch Parakeet final on release** (bench engine `nemotron-560+unified`)                                                                                                                                                                                                                                       | Measured on Linux (§6): Parakeet TDT v2 batch 3.05 % vs Nemotron 560 4.06 % WER (−25 % errors; test-clean 2.84 → 2.16 %). On a quick release, the realistic case, it is 2.70 % vs 4.33 % (−38 %), and last-word misses drop from 12.0 % to 4.7 %: the batch pass also fixes the cut-off last word. FluidAudio's Unified batch on ANE: 2.71 → 1.68 % (−38 %), with full punctuation | +~600 MB model set (signed path); one batch encode on release (~60 ms for a 7 s dictation at ~120× RTFx on ANE; 440 ms p50 on the box CPU); two models resident                                                                                             | bench `--engines nemotron-560,nemotron-560+unified`: wer, fwer, finalDecode p90                                                                                                                                                            |
| 2    | **Protect the last word: pad 1120 ms instead of 640 (keep the pad; "drop the pad" is refuted).** Proposed to Bobs (review §12.31 a)                                                                                                                                                                                                                   | Measured (§6), quick release: pad 1120 halves last-word misses, 12.0 → 6.3 %, and brings WER from 4.33 % back to 4.07 %, the level of a slow release (4.06 %). A 150 ms tail instead gets 12.0 → 9.0 % (WER 4.21 %). Dropping the pad or relying on the finish padding alone: 37-67 % misses                                                                                       | one more 560 ms chunk decoded on release (~14 ms on ANE, to be measured); talk's `HelperTail.pad_ms` must follow (`tests/hardening27.rs` pins it to the Swift default)                                                                                      | bench sweep `shipped` vs `pad-1120` (and `tail-150`, `pad-0`): tailWordsLost + releaseToStopped p50/p90 on Jared's clips                                                                                                                   |
| 3    | **Resampler: band-limited (AVAudioConverter, or a windowed sinc) instead of linear** (was rank 5; cycle 37, ADR 0115). Proposed to Bobs (review §12.34)                                                                                                                                                                                               | Measured (§9): no change on clean speech; in noise -16 % errors (white, 15 dB) and -26 % (8-20 kHz, 15 dB), both resolved; with it, 8-20 kHz noise costs nothing measurable                                                                                                                                                                                                        | tiny CPU cost on the audio thread; the converter holds a few ms of audio that the release drain must flush; a behaviour change in Bobs's capture code                                                                                                       | bench `--resample` linear vs converter on 48 kHz recordings, clean and noise-15db: wer; releaseToStopped p90                                                                                                                               |
| 4    | **Head: warm the mic while the hotkey is armed (or `warmMic` on by default with the 300 ms pre-roll)**, at least for Bluetooth inputs                                                                                                                                                                                                                 | Stops first-word clipping. Not measured yet (LibriSpeech clips start with silence); bench `cold-mic-*` gives the size on Jared's Mac                                                                                                                                                                                                                                               | mic indicator stays lit; battery; privacy perception                                                                                                                                                                                                        | bench `cold-mic-150/300` vs `warm-preroll-300`: headWordsLost; log `preroll_ms`                                                                                                                                                            |
| 5    | ~~Cleanup: no "." on short (≤ 3 word) dictations; keep the repeats rule off digits and intentional doubles~~ **Done** in cycle 32 (`cda052c`, ADR 0101), with the dot-spacing fix (N159-N161)                                                                                                                                                         | fewer manual fixes on short replies, search boxes, PINs, domains and file names; 44 hand-written vectors                                                                                                                                                                                                                                                                           | a style change users will notice                                                                                                                                                                                                                            | `echo_score` cleaned vs raw formatted WER and exactAfterCleanup on the short clips (03-yes, chat prompts)                                                                                                                                  |
| 6    | Nemotron 1120 ms tier for long dictations                                                                                                                                                                                                                                                                                                             | Punctuation kept on long dictations (FluidAudio #687). Not a WER lever: with a pad that covers its chunk (1280 ms) it scored 3.93 % vs 4.02 % for 560 ms at pad 1120 on Linux (FluidAudio 2.71 → 2.58 %). The 4.35 % at pad 640 was the pad being shorter than one 1120 chunk                                                                                                      | partials every 1.1 s, so the pill feels slower; a separate model set                                                                                                                                                                                        | bench engine `nemotron-1120` (`--extra`): wer, fwer, firstPartial                                                                                                                                                                          |
| 7    | Input / segment normalisation for quiet mics, **quiet-mic only** (§9 Nemotron; §15 Parakeet, ADR 0127). Always-on per-segment peak **rejected**. Gated peak (peak < 0.1 → 0.9, cap 10×) **landed off by default** (`e5818b3`, `QRTZ_ECHO_SEG_PEAK=on`); adds no phantom words on non-speech tails (§16, ADR 0131)                                     | Parakeet whole-clip is already robust at ×0.1 (+2.4 % relative); gated segment peak recovers ×0.03 (4.15 → 2.94) at zero cost on normal (2.57)                                                                                                                                                                                                                                     | always-on segment peak hurts normal (+11 % errors, 2.57 → 2.85); ×0.01 still ~8 % (VAD over-segments)                                                                                                                                                       | `talk latency` `level_p90_dbfs` (§10); Mac: `echo-bench` quiet/far vs normal, WER + `level_p90_dbfs`; Linux: `seg_eval.py --vad helper --segnorm --seggate 0.1`                                                                            |
| 8    | Decode-time vocabulary biasing                                                                                                                                                                                                                                                                                                                        | names and jargon right the first time                                                                                                                                                                                                                                                                                                                                              | needs a different model. Nemotron 3.5 multilingual scored 6.59 % on English (§6, auto language) and 5.72 % with English forced (§9, still +41 % errors vs the shipped model), so CTC boosting (Unified/TDT) is the likelier route, plus a signed conversion | later; measure on a names clip set                                                                                                                                                                                                         |
| 9    | **Re-punctuate each pause cut from a short window decoded across it** (3 s before, 2 s after, background; formatting, not WER; §12, ADR 0120). **Landed off by default** (`faff542`, `QRTZ_ECHO_CUT_WINDOW=on`); checked as landed in §13 (ADR 0121)                                                                                                  | Measured (§12): false sentence ends at cuts 99 → 24 against the whole-clip decode, formatted diff 7.46 → 6.10 %; with the cleanup rule already landed (89b97ab) the talk side alone is 99 → 58. As landed, through the cleanup (§13): 58 → 23, formatted diff 7.34 → 6.23 %                                                                                                        | one extra ~5.6 s decode per pause cut in the background (252 ms box CPU p50, tens of ms on ANE); 22 vs 5 sentence ends lost against the whole clip                                                                                                          | Linux `seg/helper_win.py` (as landed, §13); Mac: `echo-bench.sh --clips` on Jared's long clips, `ends vs whole` of `parakeet-segmented-win` vs `parakeet-segmented`                                                                        |
| 10   | **Drop a short release tail Parakeet isn't sure of: mean token confidence < 0.6, at most 4 words** (N169, §16, ADR 0131). **Landed off by default** (`63aab4c`, `QRTZ_ECHO_TAIL_CONF=0.6`); as landed on Linux 70 / 122 phantom tails, 0 / 174 real (§17, ADR 0132). Default-on only with the gated peak (N171) and after N170 (sentinel confidences) | Measured (§16): about 7 % of decoded room-tone-only tails get phantom words ("Yeah.", "Shit,"), gate or not; floor 0.6 drops 76 / 122 of them and 0 / 161 real tails (long, short replies, single words)                                                                                                                                                                           | a real one-word tail Parakeet is unsure of (one-word p5 0.72 on Linux; thin set, 6); Core ML confidences differ from ONNX int8, so calibrate                                                                                                                | Mac, floor and gate both on (N171): `echo-bench.sh --clips --tail-conf 0.6` (runs `QRTZ_ECHO_SEG_PEAK=on`, shipped + quiet-0.1) with clips that end in 1-2 s of room / breath / typing, plus short replies; `tail_conf=` in the timing log |

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
After a day of normal use, `node scripts/echo-timing-report.mjs` (ADR 0156; reads `~/Library/Logs/qrtz`, `--since
YYYY-MM-DD`, `--json`) summarizes both lines into the tables below; its output is the most useful thing to send back.

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

## 11. Echo on Parakeet: segmenting long dictations (ADR 0118)

Decision (Jared, 2026-10-18): echo's recogniser is Parakeet TDT v2 English, batch, resident from app start,
segmented while the key is held. The pill shows no text, so nothing needs a live preview. Nano keeps Nemotron
streaming (it loads only for a recording); Apple SpeechAnalyzer is echo's fallback. Short clips (§6): Parakeet
3.05 % WER vs Nemotron 560 ms + pad 4.06 %, last word missed 5.0 %, CPU RTF 0.077. The helper side landed in 90ae8ba (Bobs, `Segmented.swift`); his
own run of it is in §5 (2.55 % segmented vs 2.83 % whole clip). This section is the independent check of that rule.

### The eval (Linux, `linux-crosscheck/seg/`)

40 long dictations, 20–60 s (mean 39 s, 4631 words), half test-clean and half test-other: consecutive utterances
of one LibriSpeech chapter (one speaker), trimmed to speech ± 60 ms, natural gaps of 0.25–1.1 s, 0.05 s after the
last word (a quick release). Each config segments with the streaming detector below, decodes every segment with
Parakeet TDT v2 int8 (sherpa-onnx offline, 640 ms pad on every segment), and joins the words. "Errors ≤ 0.5 s from
a cut" counts alignment errors whose word lies within 0.5 s of a cut (for `whole`, of the chosen rule's cuts).
"Audio decoded after release" is the last segment plus its pad: the only decode left at the release, since the
background decoder never had a queue (queued at release 0 in every row with a cap ≥ 8 s). Release CPU in bold was
re-timed back to back on a quiet box (sherpa-onnx, 6 threads, RTF 0.05); `~` rows are their audio at that RTF.
The Mac's Neural Engine is several times faster; only audio seconds and ratios carry over.

| Row                  | Config                                                                        |    WER | errors | last word missed | segments / cap cuts | errors ≤ 0.5 s from a cut | audio decoded after release p50 / p90 / max | release CPU ms p50 / p90 / max |
| -------------------- | ----------------------------------------------------------------------------- | -----: | -----: | ---------------: | ------------------: | ------------------------: | ------------------------------------------: | -----------------------------: |
| `whole`              | whole clip                                                                    | 2.96 % |    137 |            7.5 % |           1.0 / 0.0 |             15 / 299 cuts |                        43.2 / 60.3 / 65.7 s |         **2026 / 3206 / 5351** |
| `s300-c15-o0`        | **chosen** (= Bobs's defaults): 300 ms, 15 s cap, min segment 2 s, no overlap | 2.74 % |    127 |           10.0 % |           8.5 / 0.3 |             16 / 299 cuts |                         5.6 / 11.2 / 14.6 s |            **275 / 550 / 619** |
| `s300-c15-o0-min0`   | same, no minimum segment                                                      | 2.92 % |    135 |           10.0 % |          15.9 / 0.3 |             32 / 597 cuts |                         4.6 / 10.6 / 14.6 s |               ~230 / 530 / 730 |
| `s200-c15-o0`        | 200 ms pause                                                                  | 2.83 % |    131 |           12.5 % |          9.3 / 0.23 |             20 / 332 cuts |                         4.7 / 10.8 / 14.6 s |               ~235 / 540 / 730 |
| `s500-c15-o0`        | 500 ms pause                                                                  | 2.66 % |    123 |           12.5 % |          6.6 / 0.57 |              8 / 223 cuts |                         7.0 / 13.0 / 14.7 s |               ~350 / 650 / 735 |
| `s300-p200a6-c15-o0` | 300 ms, 200 ms once the segment is 6 s                                        | 2.79 % |    129 |           10.0 % |          8.7 / 0.25 |             16 / 307 cuts |                         5.1 / 11.2 / 14.6 s |               ~255 / 560 / 730 |
| `s300-p150a6-c15-o0` | 300 ms, 150 ms once the segment is 6 s                                        | 2.87 % |    133 |           12.5 % |          8.8 / 0.25 |             17 / 314 cuts |                          5.1 / 9.8 / 14.6 s |               ~255 / 490 / 730 |
| `s300-c10-o0`        | 10 s cap                                                                      | 2.87 % |    133 |           12.5 % |          9.1 / 0.95 |             20 / 323 cuts |                           4.6 / 8.1 / 9.7 s |            **234 / 386 / 448** |
| `s300-c8-o0`         | 8 s cap                                                                       | 2.96 % |    137 |           10.0 % |          9.8 / 1.85 |             20 / 352 cuts |                           4.5 / 7.7 / 8.1 s |            **211 / 346 / 534** |
| `s300-p200a4-c10-o0` | 10 s cap, 200 ms once 4 s                                                     | 2.92 % |    135 |           12.5 % |          9.4 / 0.72 |             20 / 336 cuts |                           4.7 / 8.2 / 9.8 s |               ~235 / 410 / 490 |
| `s300-c20-o0`        | 20 s cap                                                                      | 2.66 % |    123 |           10.0 % |          8.2 / 0.05 |             14 / 289 cuts |                         5.3 / 12.9 / 17.3 s |               ~265 / 645 / 865 |
| `s300-c15-o0.5`      | 0.5 s overlap on every cut                                                    | 5.10 % |    236 |           15.0 % |           8.5 / 0.3 |             62 / 299 cuts |                         6.1 / 11.7 / 15.1 s |               ~305 / 585 / 755 |
| `s300-c15-o1`        | 1 s overlap on every cut                                                      | 6.07 % |    281 |           12.5 % |           8.5 / 0.3 |             13 / 299 cuts |                         6.6 / 12.2 / 15.6 s |               ~330 / 610 / 780 |
| `s300-c15-o0.5r`     | 0.5 s overlap, blank re-decoded from the cut                                  | 2.98 % |    138 |           10.0 % |           8.5 / 0.3 |             18 / 299 cuts |                         6.1 / 11.7 / 15.1 s |               ~305 / 585 / 755 |
| `s300-c15-o1r`       | 1 s overlap, blank re-decoded                                                 | 3.99 % |    185 |            7.5 % |           8.5 / 0.3 |             13 / 299 cuts |                         6.6 / 12.2 / 15.6 s |               ~330 / 610 / 780 |
| `s300-c15-o0.5cap`   | 0.5 s overlap after cap cuts only                                             | 3.07 % |    142 |           10.0 % |           8.5 / 0.3 |             16 / 299 cuts |                         5.6 / 11.6 / 14.6 s |               ~280 / 580 / 730 |
| `s300-c15-o1cap`     | 1 s overlap after cap cuts only                                               | 2.81 % |    130 |           10.0 % |           8.5 / 0.3 |             14 / 299 cuts |                         5.6 / 12.1 / 14.6 s |               ~280 / 605 / 730 |
| `s200-c15-o0.5`      | 200 ms, 0.5 s overlap                                                         | 6.00 % |    278 |           15.0 % |          9.3 / 0.23 |             91 / 332 cuts |                         5.2 / 11.3 / 15.1 s |               ~260 / 565 / 755 |
| `s500-c15-o0.5`      | 500 ms, 0.5 s overlap                                                         | 3.80 % |    176 |           10.0 % |          6.6 / 0.57 |             14 / 223 cuts |                         7.5 / 13.5 / 15.2 s |               ~375 / 675 / 760 |
| `s300-c10-o0.5`      | 10 s cap, 0.5 s overlap                                                       | 5.23 % |    242 |           17.5 % |          9.1 / 0.95 |             77 / 323 cuts |                          5.1 / 8.6 / 10.2 s |               ~255 / 430 / 510 |
| `s300-c20-o0.5`      | 20 s cap, 0.5 s overlap                                                       | 4.62 % |    214 |           15.0 % |          8.2 / 0.05 |             60 / 289 cuts |                         5.8 / 13.4 / 17.8 s |               ~290 / 670 / 890 |
| `s300-c10-o0.5cap`   | 10 s cap, 0.5 s after cap cuts                                                | 3.24 % |    150 |           12.5 % |          9.1 / 0.95 |             32 / 323 cuts |                           4.8 / 8.1 / 9.7 s |               ~240 / 405 / 485 |
| `s300-c6-o0`         | stress: 6 s cap, no overlap                                                   | 3.58 % |    166 |           12.5 % |         11.2 / 3.42 |             37 / 409 cuts |                           3.5 / 5.9 / 6.6 s |               ~175 / 295 / 330 |
| `s300-c6-o0.5cap`    | stress: 6 s cap, 0.5 s after cap cuts                                         | 4.28 % |    198 |           15.0 % |         11.2 / 3.42 |             50 / 409 cuts |                           4.0 / 5.9 / 7.1 s |               ~200 / 295 / 355 |
| `s300-c6-o1cap`      | stress: 6 s cap, 1 s after cap cuts                                           | 4.23 % |    196 |           12.5 % |         11.2 / 3.42 |             27 / 409 cuts |                           4.4 / 6.0 / 7.6 s |               ~220 / 300 / 380 |

Reading it:

- **Segmenting costs no accuracy.** The chosen rule is 2.74 % vs 2.96 % whole-clip (127 vs 137 errors; the
  difference is noise either way). Errors near cuts are the same as the whole clip has at those places (16 vs 15).
- **It bounds the release.** The release decodes 5.6 s of audio at p50 and never more than one segment
  (14.6 s max) instead of the whole dictation (43.2 s p50, 65.7 s max): 275 ms instead of 2026 ms of box CPU at
  p50, 619 ms instead of 5351 ms worst.
- **The p90 tail (11 s of audio) is the long sentences.** A stretch with no 300 ms pause runs to the cap. Shortening
  the pause once a segment is long barely moves it (200 ms after 6 s: p90 11.2 s, 2.79 %; 150 ms: 9.8 s, 2.87 %).
  Only the cap does: 10 s gives p90 8.1 s (2.87 %), 8 s gives 7.7 s (2.96 %, the whole clip's), each a few errors
  worse as cap cuts land mid-sentence (0.95 and 1.85 per dictation). The p90 tail costs 550 ms of box CPU against
  386 / 346 ms at the 10 / 8 s caps, and tens of ms on the Neural Engine, so the cap stays at 15 s.
- **No overlap.** An overlap reaching back across a cut into the previous word makes things worse everywhere: 5.10 %
  (0.5 s) and 6.07 % (1 s) on every cut; 3.07 % / 2.81 % on cap cuts only; with a 6 s cap forcing 3.4 cap cuts per
  dictation 4.28 % / 4.23 % vs 3.58 % without. Two causes, both seen in the decodes: a segment that starts inside a
  word is often decoded blank by TDT (15 of ~340 segments at 0.5 s, 17 at 1 s, none without overlap), and the cut
  word is heard twice (TDT stamps it late, past the stitch point). Re-decoding a blank from its cut recovers most
  of it (2.98 %), still not below no overlap. Pause cuts land in silence and cap cuts at the quietest frame, so
  there is no word to recover.
- **300 ms, 15 s cap, 2 s minimum.** 200 ms cuts mid-phrase (2.83 %, 20 errors near cuts); 500 ms is as good
  (2.66 %) but leaves longer tails (p50 7.0 s) and twice the cap cuts; a 10 s cap triples the cap cuts (2.87 %);
  a 20 s cap breaks Parakeet's 15 s window (tail max 17.3 s). Without the 2 s minimum segment the dictation is cut
  twice as often (15.9 segments) for 2.92 %.
- Last-word misses (7.5–12.5 %) differ by one or two dictations of 40 (2.5 % each): not significant.

### The rule (helper spec for Bobs; `QrtzHelperCore/Segmented.swift`)

`SegmentRule` on main (90ae8ba) matches this; the eval confirms it independently.

1. **Detector**, 16 kHz, 20 ms frames (320 samples), frame RMS `r`. Noise floor `nf` starts at the first frame's
   RMS; `nf = 0.9·nf + 0.1·r` when `r < nf`, otherwise `nf × 1.002`. Speech level `sp` starts at 0.02. A frame is
   silent when `r < max(2.5·nf, 0.06·sp, 0.001)` (`vadMarginDb` 8, `vadSpeechRelDb` −24 and a 0.001 floor
   are the same in dB); a non-silent frame updates `sp = 0.95·sp + 0.05·r`.
2. **Pause cut:** when the silence run reaches 300 ms (15 frames) and the open segment is at least **2 s** long,
   cut in the middle of the run so far, `(i + 1 − 15/2) × 320`, and start the next segment there (`minSegmentMs`
   2000; without it 2.92 % vs 2.74 %). `minSpeechMs` 200 is fine on top. The helper's VAD floor is 0.0002 RMS; the
   eval's gaps are digital silence, so the floor makes no difference here.
3. **Cap cut:** a segment plus its pad never exceeds 15 s (your cap counts the pad: 14.36 s of audio). At the cap,
   cut at the quietest frame of the last 1.5–2 s (yours: the latest quietest 100 ms of 1.5 s; the eval used the
   quietest 20 ms frame of 2 s, and the cap rarely fires: 0.3 times per dictation).
4. **Overlap 0** on every cut (`overlapMs` 0, your default). With an override set, re-decode a segment once from its
   cut, without the overlap, when its decode is blank but it had ≥ 300 ms of speech.
5. **Pad 640 ms of zeros after every segment**, the background ones and the last (your `segmentPadMs` 640; the
   release pad goes on the last segment). The end of a background segment needs the same end context as the
   release. (`helper-protocol.md` says the pad goes on the last segment only; the code pads every one, as it should.)
6. **Stitching:** with no overlap the segments' words are appended in order. With an overlap `ov` at a cut `c`, keep
   the earlier segment's words starting before `c − ov/2` and the later one's from there (word start times), then
   drop the later segment's first word when it repeats or extends the earlier one's last (your `WordStitcher`).
7. **Background decoder:** one serial chain; a segment is enqueued when it is cut and has speech. At the release, wait
   for the chain, then decode the last segment (skip it when it has no speech). On the deadline, answer with the
   stitched segments decoded so far and set `final_timed_out`.
8. **Release deadline, `SegmentedTail.budgetMs(audioMs:)`**: `tailMs + drainLimitMs(queuedMs: 0) + min(1500 +
   (min(audio, cap − pad) + overlap + pad + min(max(0, audio − (cap − pad)), cap − pad)) × 100 / 1000,
   max(1500, maxWaitMs − drainLimitMs(queuedMs: 0)))`, i.e. the last segment and at most one more still decoding.
   qrtz-talk's `HelperTail::segmented_budget_ms` is this formula (N130: talk's wait is it + pad + slack). **Open on
   the helper:** its release deadline is still `DictationTail.budgetMs(audioMs:queuedMs:)`, the streaming formula
   (about 21.5 s for a long dictation), so talk may now go ahead before the helper gives up on a hung decode. The
   segmented recogniser should take this formula as its deadline so the two stay equal. `tests/hardening27.rs` reads `SegmentRule`'s
   `capMs`, `overlapMs` and `segmentPadMs` defaults from `Segmented.swift` once it is on main.
9. **Wire and log:** `dictation.stopped` carries `asr`, `segments` and `tailDecodeMs` (your fields, in
   `protocol.ts`/`contracts/` now). The `echo timing` line carries `segments=`, `bg_segments=`, `cap_cuts=`,
   `tail_decode_ms=` and `backlog_wait_ms=` (yours). No text in either.

### qrtz-talk (this cycle)

Once this helper process has answered a dictation with `segments`, talk's stop wait is the segmented budget plus
pad and slack: it stops growing at two segments of audio, 6.9 s for a 60 s dictation (the streaming bound was
23.7 s) and 4.6 s for 5 s (was 19.5 s). The first dictation after a helper start, or after a stop without
`segments` (a fallback recogniser), keeps the streaming bound. The `talk latency` line adds `asr=`, `segments=` and
`tail_decode_ms=`. Recordings (nano) are untouched. Tests: `crates/qrtz-talk/tests/segmented_tail.rs`,
`hardening27.rs::the_segmented_copy_matches_the_helper_segment_rule`.

### Bench on the Mac

`echo-bench` engines `parakeet-seg-<pause>-<cap>[-<overlap>]` (Bobs's `EchoBench.swift`): run
`parakeet-seg-300-15` (the rule), `parakeet-seg-500-15` and the whole clip on Jared's own long dictations
(`echo-clips.tsv`). The Mac numbers that matter are `tail_decode_ms` (the release decode, expected well under
the box's 275 ms p50) and WER against the whole clip.

## 12. Cycle 38: sentence ends at segment cuts (ADR 0120, N167)

Segmenting (§11) costs no WER, but each segment is decoded alone with 640 ms of zero pad, and Parakeet ends almost
every one with a sentence end. `WordStitcher` (`Segmented.swift:235,258`) appends the words as decoded, so the dot
goes into the text, and talk's cleanup capitalized the word after it. Measured on the 40 long dictations of §11
with cached decodes only (`linux-crosscheck/seg/punct.py`), rule s300-c15: 245 of 283 aligned cuts end in `.?!`
against 152 at the same place in the whole-clip decode; 94 are sentence ends the whole clip doesn't have (70 at
pauses inside an utterance), and 64 commas become dots. LibriSpeech has no punctuation, so the whole-clip decode
(Parakeet with full context) is the reference: not ground truth, but the text echo would type without segmenting.

Talk side, landed (`89b97ab`, ADR 0120): a lone "." followed by a lowercase word is dropped (abbreviations keep it
and don't capitalize the next word), for cased recognisers only. Through the cleanup on both sides
(`punct_cleanup.py`):

| Rule s300-c15  | ends agreeing | false ends | lost ends | formatted diff vs whole |
| -------------- | ------------: | ---------: | --------: | ----------------------: |
| cleanup before |           231 |         99 |         4 |                  8.07 % |
| cleanup after  |           221 |         58 |        13 |                  7.34 % |

(500 ms pause: 60 → 31 false, 10 → 17 lost, 6.93 → 6.43 %.)

Helper side, proposed (review §12.35): decode a window across each pause cut once its "after" part is captured,
and take only the cut word's trailing punctuation and the next word's case from it (`cutwin.py`, raw text, no
cleanup, vs the raw whole clip: 229 agreeing / 99 false / 5 lost / 7.46 % without it):

| Window (before / after the cut) | ends agreeing | false ends | lost ends | formatted diff | window CPU ms p50 / p90 (box) |
| ------------------------------- | ------------: | ---------: | --------: | -------------: | ----------------------------: |
| 2 s / 1 s                       |           221 |         46 |        13 |         6.60 % |                     194 / 261 |
| **3 s / 2 s**                   |           212 |     **24** |        22 |     **6.10 %** |                     252 / 373 |
| 4 s / 3 s                       |           210 |         17 |        24 |         5.88 % |                     307 / 378 |

3 s / 2 s halves the disagreements with the whole clip (104 → 46) for one background decode of ~5.6 s per cut;
4 s / 3 s adds little (41). The window decode is background work (the cut is ≥ 2 s old by then), so it doesn't
touch the release unless a cut is within 2 s of it; then the cleanup rule is the fallback.

Helper side, implemented (review §12.35 (a), off by default until the Mac gate): `SegmentRule.cutWindow`
(`QRTZ_ECHO_CUT_WINDOW=on`, sizes `QRTZ_ECHO_CUT_WINDOW_BEFORE_MS` = 3000 / `_AFTER_MS` = 2000). Once a pause cut
has the "after" part captured, `SegmentedTranscriber` queues the window `[max(previous cut, c − before), c + after]`
plus the 640 ms pad on its serial chain; `WordStitcher.repunctuate` then copies only the cut word's trailing
`[.?!,;:]+` and the next word's first-letter case, when the window's last word before `c` and first at or after
`c − 0.1 s` match the stitched words on both sides and are adjacent (as `cutwin.py`). At the release, windows
not yet started are skipped and not waited for (`windows_skipped`); one already decoding finishes first, because
the decoder is serial. Timing line: `cut_windows= window_fixes= windows_skipped=` when windows ran. Bench:
`echo-bench.sh` runs `parakeet-segmented-win` next to `parakeet-segmented` and `parakeet-whole`, and each
summary row has `sentenceEndsVsWhole` (agreeing / spurious / lost against the whole clip's raw text) plus
`cutWindows` / `windowFixes` / `windowsSkipped`. Since ADR 0121 cap cuts get a window too, and
`sentenceEndsVsWhole` also counts `spuriousWholeContinues` (strict: a false end where the whole clip's next word
starts lowercase; printed as `strict` in the `ends vs whole` line). Gate: strict spurious halved against
`parakeet-segmented`, WER unchanged, and p90 `tailDecodeMs` / `backlogWaitMs` not higher; then turn `cutWindow`
on by default. No window for the last cut at the release, and the release budget is unchanged.

On the Mac: dictate a long paragraph with natural pauses mid-sentence (e.g. "I went to the store, and then,
after a while, I came back home"), with cleanup on. Before `89b97ab` the pauses come out as ". Then" / ". After";
now a lowercase continuation is joined. A capitalized one ("store. And then") is what §12.35 fixes.

## 13. Cycle 39: the helper's cut windows as landed (ADR 0121)

Bobs landed §12's windows in `faff542` (off by default). Three differences from `cutwin.py`: pause cuts only, a
window only once 2 s after the cut is captured (so a cut within 2 s of the release has none), and the segment
before the cut must have speech. `linux-crosscheck/seg/helper_win.py` replays that exact rule and
`WordStitcher.repunctuate` on the 40 long dictations of §11 (cached decodes), then talk's `ruleCleanup` on both
sides. "Strict" counts a false end only where the whole clip's next word is lowercase: about a third of the plain
count is the whole-clip decode itself dropping a sentence end on a 40-60 s clip ("happy life He was").

| Variant                   | raw: false / strict / lost |   raw diff | cleanup: false / strict / lost | cleanup diff |
| ------------------------- | -------------------------: | ---------: | -----------------------------: | -----------: |
| segmented, no windows     |                99 / 76 / 5 |     7.46 % |                   58 / 36 / 13 |       7.34 % |
| **as landed (`faff542`)** |           **30 / 17 / 23** | **6.40 %** |               **23 / 11 / 23** |   **6.23 %** |
| + windows on cap cuts     |               28 / 15 / 23 |     6.27 % |                   23 / 11 / 23 |       6.13 % |
| + a window at the release |               27 / 14 / 23 |     6.32 % |                    21 / 9 / 23 |       6.15 % |
| idealised (`cutwin.py`)   |               24 / 11 / 22 |     6.10 % |                    21 / 9 / 22 |       5.97 % |

As landed: 283 windows on 287 pause cuts, 120 changed the cut, 28 found no adjacent matching pair, 4 skipped near
the release, 12 cap cuts skipped. A window at the release would cost 248 / 266 / 272 ms box CPU (p50 / p90 / max)
on the release for 2 false ends on 4 of 40 dictations: rejected. Cap-cut windows: −0.10 pp, optional (review
§12.36 a). The Mac gate should use the strict count (§12.36 b). Lost ends 13 → 23 are the window's "," where the
whole clip has "." ("the wound, the quiver"); a wider window doesn't fix them (§12).

On the Mac: record 3-5 long dictations (30-60 s) with natural pauses mid-sentence into `~/qrtz-echo-bench/my-clips`
(with `.txt` references, §5), run `bash native/qrtz-helper/scripts/echo-bench.sh --clips ~/qrtz-echo-bench/my-clips`,
and compare the `ends vs whole` lines of `parakeet-segmented-win` and `parakeet-segmented` in the summary.

## 14. Cycle 40: pause length doesn't break cut-window ties (ADR 0123)

The windows' lost sentence ends (13 → 23 after cleanup, §13) are mostly the window decoding "," where the segment
and the whole clip have ".". `linux-crosscheck/seg/pause_keep.py` replays the helper as landed in `1c73921`
(windows on pause and cap cuts), measures the silence run at each cut with the segmenter's own detector, and tries
two tie-breakers. One keeps the segment's end when the pause is ≥ P. The other drops the window's end when the
pause is < Q. Through the cleanup on both sides, against the whole clip:

| Rule                  | false / strict / lost | formatted diff |
| --------------------- | --------------------: | -------------: |
| as landed (`1c73921`) |          23 / 11 / 23 |         6.13 % |
| keep ≥ 700 ms         |          36 / 18 / 13 |         6.10 % |
| keep ≥ 800 ms         |          30 / 16 / 14 |         5.97 % |
| keep ≥ 1000 ms        |          26 / 13 / 19 |         6.02 % |
| drop < 400 ms         |          22 / 11 / 39 |         6.80 % |

Rejected. The best rule trades 9 lost ends (a comma where a period could go) for 5 strict false ends (a capital
in the middle of a sentence) and gains only 0.16 pp. The set's pauses between utterances are synthetic
(0.25-1.1 s), so even that gain may not carry over to real speech. The cut windows stay as landed, and so does the
gate (§12). The baseline matches §13's "+ windows on cap cuts" row exactly, so the replay is right about the
helper. On the Mac the `lost` column of `parakeet-segmented-win` in `echo-bench.sh --clips` tells whether the lost
ends matter on Jared's speech.

## 15. Cycle 41: helper VAD on Linux; always-on segment peak rejected (ADR 0127, N168)

`linux-crosscheck/seg/helper_seg.py` ports the helper's `PauseSegmenter` (vadMinRms 0.0002, minSpeechMs 200,
smoothed cap search, pad counted in the cap). `seg_eval.py run --vad helper` uses it; `--vad sketch` keeps the
earlier tables. Quiet-mic and per-segment peak rows below all use the helper VAD.

Whole-clip Parakeet TDT v2 int8 (300 LibriSpeech) is far more level-robust than Nemotron (§9: quiet ×0.1 was
+23 % errors). Quiet ×0.1 is only +2.4 % relative (2.86 → 2.93). Peak recovers quiet fully and does not hurt
normal on the whole clip:

| name         | scale | norm             | werAll | tailLost | headLost |
| ------------ | ----: | ---------------- | -----: | -------: | -------: |
| pk-base      |     — | —                |   2.86 |      4.7 |      2.0 |
| pk-peak      |     — | peak→0.9, cap 10 |   2.81 |      4.7 |      2.0 |
| pk-agc       |     — | agc              |   2.82 |      5.0 |      3.3 |
| pk-q01       |  ×0.1 | —                |   2.93 |      4.7 |      1.7 |
| pk-q01-peak  |  ×0.1 | peak             |   2.79 |      4.7 |      1.7 |
| pk-q01-agc   |  ×0.1 | agc              |   2.74 |      4.3 |      1.7 |
| pk-q003      | ×0.03 | —                |   3.12 |      5.3 |      3.3 |
| pk-q003-peak | ×0.03 | peak             |   2.79 |      4.3 |      2.7 |

Segmented (40 long dictations, helper VAD, s300-c15). Always-on per-segment peak to 0.9 hurts normal
(2.57 → 2.85 / +11 % errors) and is **rejected**. Gating on `peak < 0.1` leaves normal alone and recovers
×0.03 (4.15 → 2.94). At ×0.01 the VAD over-segments (cuts 286 → 407) and WER stays ~8 % even with peak — a
floor, not a gain bug. Candidate for Bobs (§12.37); rank 7 stays quiet-mic-only via `level_p90_dbfs`.

Helper side, implemented (review §12.37, off by default): `SegmentRule.segPeak` (`QRTZ_ECHO_SEG_PEAK=on`). In
`SegmentedTranscriber` every decode (background segment, cut window, release tail) goes through `SegmentPeak`:
peak abs below 0.1 → scaled so the peak is 0.9, gain at most 10×; anything louder is untouched. It works on the
decode copy only, after the cut, so `PauseSegmenter`'s VAD, nano (Nemotron streaming) and the shared mic stream
never see it. Never always-on (the ungated row above: +11 % errors on normal speech). Timing line `peak_gained=`
(segments and tail scaled, windows not counted; only with the gate on), bench `peakGained`; `echo-bench.sh
--seg-peak` runs `parakeet-segmented` vs `parakeet-segmented-peak` on `shipped`, `quiet-0.1` and the new
`quiet-0.03`. Mac gate: `shipped` WER unchanged (`peakGained` ≈ 0 there), `quiet-0.03` WER down.

| name             | scale | segnorm  | gate      |   WER | errors |
| ---------------- | ----: | -------- | --------- | ----: | -----: |
| c41-seg          |     — | no       | —         |  2.57 |    119 |
| c41-seg-pk       |     — | yes →0.9 | always    |  2.85 |    132 |
| c41-seg-g01      |     — | yes →0.9 | peak <0.1 |  2.57 |    119 |
| c41-whole        |     — | no       | —         |  2.96 |    137 |
| c41-q01-seg      |  ×0.1 | no       | —         |  2.94 |    136 |
| c41-q01-seg-pk   |  ×0.1 | yes      | always    |  2.72 |    126 |
| c41-q01-seg-g01  |  ×0.1 | yes      | <0.1      |  2.81 |    130 |
| c41-q003-seg     | ×0.03 | no       | —         |  4.15 |    192 |
| c41-q003-seg-pk  | ×0.03 | yes      | always    |  2.92 |    135 |
| c41-q003-seg-g01 | ×0.03 | yes      | <0.1      |  2.94 |    136 |
| c41-q001-seg     | ×0.01 | no       | —         | 14.36 |    665 |
| c41-q001-seg-pk  | ×0.01 | yes      | always    |  8.25 |    382 |
| c41-q001-seg-g01 | ×0.01 | yes      | <0.1      |  8.29 |    384 |

On the Mac: record quiet/far mic clips (and matching normal-level ones) into `~/qrtz-echo-bench/my-clips`, run
`bash native/qrtz-helper/scripts/echo-bench.sh --clips ~/qrtz-echo-bench/my-clips`, and compare WER plus the
`talk latency` `level_p90_dbfs` of the quiet set against the normal set before turning a gated peak on.

## 16. Cycle 42: non-speech release tails; the gate is fine, a confidence floor for phantom words (ADR 0131, N169)

The gated peak as landed (`e5818b3`) checked against what it could make worse: a release tail with no words in it
(the speaker stopped, then breathed, typed or waited before letting go). `linux-crosscheck/seg/nonspeech.py`:
one LibriSpeech utterance, 500 ms pause (the helper cuts there), 1.5 s of a sound as the tail, quick release; 60
dictations × breath / keys / click / room tone × peak 0.005–0.09 × normal and ×0.1 mic; helper PauseSegmenter;
Parakeet TDT v2 int8 plain vs gated (peak < 0.1 → 0.9, cap 10×).

| set (decoded noise-only tails)                  | tails | with words, plain | with words, gated | WER plain → gated |
| ----------------------------------------------- | ----: | ----------------: | ----------------: | ----------------- |
| breath / keys / click after true silence        |   649 |           0 (0 %) |         1 (0.2 %) | 2.91 → 3.01       |
| room tone tail (strict LibriSpeech room tone)   |   229 |        16 (7.0 %) |        11 (4.8 %) | 3.44 → 3.32       |
| room tone under everything (`--bg`), all sounds |   677 |        51 (7.5 %) |        45 (6.6 %) | 3.92 → 3.67       |

So the gate adds nothing here. What it found instead (N169): Parakeet writes a short phrase on about 7 % of decoded
room-tone tails, gate or not ("Yeah.", "Shit,", "How many of you have", "Should I have a picture of"), and those
words get inserted. Requiring more VAD speech frames in the tail doesn't help: room tone passes as speech, and
200 ms removes only 8 of 51 while losing 1 of 10 one-word tails (`tailguard.py`). Parakeet's own token confidence
does separate them (`confguard.py`, `confwords.py`; mean per-token probability of the tail's words):

| tails                         |   n | p5 / p10 / p50     |
| ----------------------------- | --: | ------------------ |
| hallucinated (noise only)     | 122 | 0.36 / 0.39 / 0.55 |
| real, release tail of 40 long | 119 | 0.87 / 0.90 / 0.97 |
| real, short utterance tail    |  36 | 0.85 / 0.88 / 0.95 |
| real, one word tail           |   6 | 0.72 / 0.73 / 0.79 |

Floor 0.6 drops 76 / 122 hallucinated tails and 0 / 161 real ones. Floor 0.7 drops 99 / 122, still 0 real ones,
but it is close to the one-word p5. Proposed to Bobs as rank 10 (review §12.38): 0.6, tails of at most 4 words,
off by default. On the Mac: record clips that end with 1–2 s of room, breath or typing before the release, plus
short replies, and read `tail_conf=` in the timing log. The Core ML (ANE) confidences need their own calibration.

Helper side, implemented (review §12.38 (a), off by default): `BatchResult.confidence` (ParakeetBatch fills it from
`ASRResult.confidence`) and `SegmentRule.tailConfFloor` (`QRTZ_ECHO_TAIL_CONF=0.6`, 0 < floor ≤ 1, else ignored).
After the release tail's decode, a tail of at most 4 words below the floor isn't stitched; the earlier segments'
text stays. The timing line has `tail_conf=` whenever the decode reports a confidence (so the floor can be set from
real values with it off) and `tail_dropped=1` when it acted; bench `tailConf` / `tailDropped` per clip, and
`tailConf` stats, `tailConfMin` and `tailDropped` (clips) per summary row. Mac gate (§12.38 (b)): `bash
native/qrtz-helper/scripts/echo-bench.sh --clips ~/qrtz-echo-bench/my-clips --tail-conf 0.6` runs
`parakeet-segmented` without and with the floor and prints each dropped tail with its `tail_conf` (since §17
both runs have `QRTZ_ECHO_SEG_PEAK=on`: default-on means both env vars together).

## 17. Cycle 43: the tail floor as landed; sentinel confidences, and the floor needs the gate (ADR 0132, N170, N171)

`63aab4c` (Bobs) checked against §12.38 and replayed line by line on Linux (`linux-crosscheck/seg/tailfloor.py`:
`dropsTail`, the `QRTZ_ECHO_TAIL_CONF` parsing and FluidAudio's `calculateConfidence`, selfcheck equal to
`TailConfTests`). The semantics match: strict `<`, 1–4 whitespace words of the tail decode, no confidence kept,
release tail only, no dangling punctuation (the last cut's window is skipped when the tail goes).

| floor (landed: ≤ 4 words) | phantom tails dropped (of 122) | phantom words removed (of 216) | real tails lost, gained (of 174) | room tail WER plain / gated | room under all WER plain / gated |
| ------------------------: | -----------------------------: | -----------------------------: | -------------------------------: | --------------------------- | -------------------------------- |
|                       off |                              0 |                              0 |                                0 | 3.44 / 3.32                 | 3.92 / 3.67                      |
|                       0.5 |                             39 |                             60 |                                0 | 3.39 / 3.31                 | 3.82 / 3.61                      |
|                   **0.6** |                         **70** |                        **108** |                            **0** | **3.32 / 3.19**             | **3.77 / 3.57**                  |
|                       0.7 |                             93 |                            146 |                                0 | 3.26 / 3.15                 | 3.72 / 3.54                      |
|                      0.75 |                            107 |                            166 |           2 (1 long, 1 one word) | 3.21 / 3.08                 | 3.71 / 3.52                      |

Cycle 42's 76 counted the 6 phantom tails of 5+ words ("Should I just say that?", 0.29), all under 0.6; the
4-word cap keeps them. Real tails: 132 long-tail decodes, 36 short replies, 6 one-word tails, every quiet one
gained (cycle 42's sets).

The default is the gate off, so the quiet real tails decoded plain (`tailfloor.py --ungated`):

| quiet real tails, plain (no gated peak) |           n | dropped at 0.6 | dropped at 0.7 | which                                          |
| --------------------------------------- | ----------: | -------------: | -------------: | ---------------------------------------------- |
| ×0.1: long / short / one word           | 40 / 14 / 1 |      0 / 0 / 1 |      0 / 0 / 1 | "You" 0.54 (gained: 0.75)                      |
| ×0.03: long / short / one word          |  40 / 5 / 2 |      1 / 1 / 1 |      1 / 1 / 1 | "Oh" 0.54, "King and Shelby." 0.55, "You" 0.57 |

Phantoms go either way (plain 39 / 66 dropped at 0.6, gated 31 / 56). So the floor is on by default only with
`QRTZ_ECHO_SEG_PEAK=on` (N171). And FluidAudio reports 0.1 for an empty decode and 0.5 when token confidences
are missing (N170): the second is under the floor, the first fills `tail_conf=` and the bench's `tailConfMin`
with empty decodes (213 of 229 decoded room-tone tails on Linux are empty). Proposal §12.39: nil for both.
Talk side `86c50fe`: `tail_conf=` / `tail_dropped=true` on the `talk latency` line once `dictation.stopped`
carries `tailConf` / `tailDropped` (§12.39 (c)).

Mac gate (§12.39 (b)): `QRTZ_ECHO_SEG_PEAK=on bash native/qrtz-helper/scripts/echo-bench.sh --clips
~/qrtz-echo-bench/my-clips --tail-conf 0.6`, clips ending in 1–2 s of room / breath / typing plus short replies,
some recorded quiet (mic far away or input gain low); read the `tail conf` and `dropped tail` lines.

Helper side, implemented (§12.39): (a) N170, `BatchResult.reportedConfidence` (pure, tested) makes
`ParakeetBatch`'s confidence nil when the text is blank after trimming or `r.tokenTimings` is nil or empty, so
FluidAudio's 0.1 / 0.5 placeholders never reach the floor, `tail_conf=` or the bench's `tailConf` /
`tailConfMin`; a nil confidence keeps the tail. (b) N171, `--tail-conf f` runs both passes with
`QRTZ_ECHO_SEG_PEAK=on` on `shipped` and `quiet-0.1` (the prefix above is now implied; the floor is only shipped as
`QRTZ_ECHO_SEG_PEAK=on QRTZ_ECHO_TAIL_CONF=0.6`). (c) `dictation.stopped` carries `tailConf` (0..1, 3 places) and
`tailDropped` (only `true`, only when the floor dropped the tail), optional and additive, read by talk's `AsrInfo`
(`86c50fe`). P5: `tail_conf=` prints `%.3f`, so a 0.595 tail reads `tail_conf=0.595 tail_dropped=1`.

## 18. Cycle 44: the VAD floor in a noisy room (ADR 0136)

`PauseSegmenter` restarts each dictation at floor 0.001 and raises it ×1.002 per 20 ms frame (capped at speech
×0.1), so in a fan / AC / café room the noise counts as speech for the first part of a dictation and only the 15 s
cap cuts. `linux-crosscheck/seg/roomfloor.py`: §11's 40 long dictations with noise under everything (also the
0.3 s before the first word), SNR against the active speech level, helper PauseSegmenter port, Parakeet TDT v2 int8.

| condition   | pause / cap cuts: shipped, ×1.01 creep, 1 s min-stats | release tail p90 (s) | release CPU p90, box (ms) | WER shipped / creep / min-stats |
| ----------- | ----------------------------------------------------- | -------------------- | ------------------------- | ------------------------------- |
| clean       | 6.8 / 0.4, 8.0 / 0.2, 9.8 / 0.1                       | 10.1, 8.0, 6.1       | 442, 353, 374             | **2.53** / 2.79 / 2.85 %        |
| room, 25 dB | 1.2 / 2.1, 8.3 / 0.2, 9.2 / 0.2                       | 11.8, 6.4, 6.1       | 440, 320, 306             | **3.02** / 3.07 / 3.35 %        |
| room, 20 dB | 0.3 / 2.4, 3.7 / 1.3, 5.4 / 0.9                       | 12.5, 11.2, 9.4      | 907, 498, 390             | **3.04** / 3.41 / 3.63 %        |
| fan, 25 dB  | 6.3 / 0.8, 9.9 / 0.1, 9.8 / 0.1                       | 6.4, 6.1, 6.1        | 377, 315, 313             | 3.15 / **3.00** / 3.15 %        |
| fan, 20 dB  | 1.2 / 2.2, 8.7 / 0.4, 9.1 / 0.3                       | 12.5, 6.1, 6.0       | 523, 372, 372             | **3.15** / 3.61 / 3.48 %        |

The slow floor is the more accurate one (mean 2.98 % vs 3.18 % and 3.29 %): fewer, longer segments keep their
context, as with the 2 s minimum segment in §11. What it costs is release decode, bounded by the cap (tens of ms
on the ANE). Kept as shipped. On the Mac, a noisy room shows `cap_cuts=` 1–2 and a higher `tail_decode_ms=`;
that is expected. Revisit (carry the floor across dictations) only if `tail_decode_ms` p90 tops ~150 ms there.

## 19. Cycle 45: echo switched off mid-dictation (ADR 0147, N172)

Not a speed or accuracy change: a correctness fix in the push-key path after ADR 0146 made echo switchable at
runtime. Switching echo off while fn is held used to leave the mic open (no release could arrive); now the
dictation is cancelled at once and nothing is inserted (`38e1ae0`). On the Mac: hold fn, turn off the "echo (dictation)" switch in
Settings (modules section) while still holding, and check that the pill and the orange mic dot go away immediately and no
text appears. The ranked plan above is unchanged; its open items still wait on the helper side or the Mac bench.

## 20. Cycle 46: the module split, reviewed (ADR 0150, N173, N174)

No speed or accuracy change in the default layout: the helper's `EchoController` split (`254078a`) is a faithful
move. Two requirements for the split layout (`QRTZ_MODULE_HELPERS=split`, off by default, and qrtz-echo does not run
dictation yet): echo's own bundle needs the system audio grant or the speaker gate goes off silently (N173), and echo
must be prewarmed after a download or the first press is cold (N174). Both are on the helper side; checks are in
ADR 0150. Nothing new to test on the Mac in the default layout. The ranked plan above is unchanged.

## 21. Cycle 47: the timing report (ADR 0156, N175)

No speed or accuracy change: the module split's echo controller (`6c48502`) is reviewed, and N173 / N174 are closed
(review §55). New: `node scripts/echo-timing-report.mjs` turns the local log into this plan's inputs: per recogniser,
p50 / p90 / max of release-to-stopped, finalize, tail decode and backlog wait; the `gate=` modes; the release tail
confidence histogram against the 0.6 floor (rank 10's Core ML calibration); talk's final / insert times; and how often
the mic was quiet (rank 7). Its notes flag `tail_decode_ms` p90 over 150 ms (§18), timeouts, `gate=off-no-*` and too
few tails to calibrate. Talk's latency line now has plain stage times (`final_ms=160`, not `Some(160.0)`;
`311f89e`). No numbers from the Mac yet; the ranked plan above is unchanged.

## 22. Cycle 48: the first model download through the package manager (ADR 0160, N176)

No dictation speed or accuracy change. Echo's Parakeet set now installs through the package manager (`5c824f8`),
trying `modules.qrtz.ai` and its mirror before the pinned Hugging Face URL. When those hosts can't be reached and are
slow to fail, the first download used to wait about 14 minutes of timeouts before any byte came; now about 30 s
(`8a3b994`). Already-downloaded models are kept as they are. The ranked plan above is unchanged.

## 23. Cycle 50: the download bar counts each byte once (ADR 0165, N177)

No dictation speed or accuracy change, and no numbers from the Mac yet. When a download source fails partway
through one of Parakeet's files and the next source resumes it, the bar no longer counts the finished part again
(`4f21e28`); it can't pass 100 % early. It only matters once `modules.qrtz.ai` is live; today Parakeet comes from
Hugging Face alone. The ranked plan above is unchanged.

## 24. Cycle 51: Echo updates and the running process (ADR 0172, N178)

No dictation speed or accuracy change, and no numbers from the Mac yet. With qrtz shipping empty (`902bcab`), Echo
runs from its installed package; installing it still gives a warm first press, because Parakeet is linked before
`qrtz-echo` starts. Updating Echo from the marketplace, though, doesn't reach dictation until qrtz is relaunched: the
old process keeps running (from a package dir the cleanup already removed). Proposed to Bobs: relaunch echo after an
update, but only once no dictation is in progress (ADR 0172). The ranked plan above is unchanged.

## 25. Cycle 52: echo is bundled; the first-launch download (ADR 0175, N179)

No dictation speed or accuracy change, and no numbers from the Mac yet. With echo inside qrtz.app (`44622b3`), the
first press after Parakeet's download is still warm: the model is linked before the "installed" hook, which warms
`qrtz-echo`. But a fresh qrtz has echo already on, so the one-time "download and turn on" question never appears;
the only route to Parakeet is the settings link. Proposed to Bobs: offer the download right on echo's page (ADR 0175).
The ranked plan above is unchanged.

## 26. Cycle 53: a press before Parakeet is downloaded (ADR 0177, N182)

No dictation speed or accuracy change, and no numbers from the Mac yet. On a fresh qrtz, holding fn before Parakeet
is downloaded (on a Mac without the OS dictation assets) said "speech recognition isn't ready yet; try again in a
moment", which never helps. It now says "echo's speech model isn't downloaded yet; open echo to download it (about
464 MB)" (`4672ea4`, `79cfd70`); a recogniser that is only loading or busy still says "try again". The ranked plan
above is unchanged.

## 27. Cycle 54: speech status failures on echo's page (N183, N184)

No dictation speed or accuracy change, and no numbers from the Mac yet. If echo's page can't read the speech model's
status, it now says so with a retry, and the switch still works (`83fc45e`). With the status unknown, turning echo
on asks to download Parakeet even when it is already there (N184, proposed to Bobs: check again on the click). Module
releases in CI (ADR 0179) leave the bundled echo and its Parakeet alone. The ranked plan above is unchanged; its open
items still wait on the helper side or the Mac bench.

## 28. Cycle 55: the polish test set, two false-positive fixes, and the cleanup's cost (ADR 0182)

Polish is now first in the plan (Jared, Oct 18 10:08 PM PT: no LLM, no network, no added latency, strict improvement
only). Scored by `cargo test -p qrtz-talk --test polish_set -- --nocapture` on `spec/vectors/talk/polish.json`:

| Run                | Exact   | Edit distance      | Regressions | False-positive hits |
| ------------------ | ------- | ------------------ | ----------- | ------------------- |
| d457ff0 (baseline) | 20 / 42 | 211 chars          | -           | 14 / 42             |
| b07898b (cycle 55) | 23 / 42 | 201 chars (15.5 %) | 0           | 0 / 42              |

- N185 fixed: fillers kept "ER", "UM", "AH" and "35 mm" (they were deleted). "I spent the night in the ER with my dad."
  was typed as "I spent the night in the with my dad."
- N186 fixed: "Start a new line of products." was typed as "Start a" + line break + "Of products."
- N187 fixed: the dictionary step recompiled every pattern on every dictation, after release. Cleanup on a 495-word
  dictation with a 20-term dictionary: **22.3 ms → 1.64 ms** median (release build, box CPU); a one-line dictation
  with that dictionary was ~19 ms, now well under 1 ms. Same text out. `tests/cleanup_budget.rs` holds the 2 ms budget.
- Release-to-text otherwise unchanged: nothing in capture, segmenting, the decode schedule or Parakeet's settings moved.

Next polish candidates, ranked: (1) what Parakeet/FluidAudio already does (punctuation, capitals, digits; any
vocabulary boosting for the TDT model), checked against the FluidAudio source, as a proposal to Bobs; (2) end-of-
dictation-only spoken punctuation ("... question mark") measured against the keep set; (3) warming dictionary patterns
off the press path.

## 29. Cycle 56: an explicit "Scratch that" (ADR 0186, `fc370d3`)

| Run                | Exact   | Edit distance      | Regressions | False-positive hits |
| ------------------ | ------- | ------------------ | ----------- | ------------------- |
| b07898b (cycle 55) | 23 / 42 | 201 chars (15.5 %) | 0           | 0 / 42              |
| fc370d3, same 42   | 25 / 42 | 133 chars (10.2 %) | 0           | -                   |
| fc370d3, 46 cases  | 29 / 46 | 133 chars (9.35 %) | 0           | 0 / 59              |

- "Let's meet on Tuesday. Scratch that, let's meet on Wednesday." is now typed as "Let's meet on Wednesday." Only
  when "Scratch that" is its own sentence after a clean sentence end and is followed by "," or "." and the new words;
  the sentence before it goes. Mid-sentence ("scratch that off my list"), "Scratch that itch", "Scratch that, please",
  dotted words or abbreviations at the edges ("3 p.m.", "Jr."), and nothing before or after are all left as spoken.
  17 new keep vectors against 6 positive.
- Cost: a few microseconds (0.0017 ms on 484 words without the words, 0.0037 ms with one correction, release); the
  2 ms chain budget holds. Nothing in capture, segmenting, the decode schedule or Parakeet's settings moved.

Next polish candidates, ranked: (1) what Parakeet/FluidAudio already does (punctuation, capitals, digits, vocabulary
boosting for the TDT model), checked against the FluidAudio source, as a proposal to Bobs; (2) no "." after a list
whose lines are all short (p12); (3) dictation-final "question mark" / "exclamation point" only where it can't be the
noun (the keep set already has "put a question mark"); (4) warming dictionary patterns off the press path.

### Production-readiness backlog (Bobs, Oct 18 10:13 PM PT)

Worked one per cycle when no polish item is ready. P1 the `local_models::speech_calls_scoped_to_one_module_
touch_only_its_models` timeout: **fixed `a8da381`** (cycle 69, ADR 0209; a lost resume, not a flake; section 41);
P2 model download pause/resume/background/crash recovery with sha256: crash recovery **fixed `b88bae3`** (cycle 72, ADR 0218, N190-N192; section 44; N193 open, Bobs); P3 no network or a failed index refresh never
blocks dictation with an installed model: **fixed `de03608`** (cycle 70, ADR 0210, N188; section 42), and its follow-up N189 **fixed** (`b464db9` store side, Bobs; `7cf1272` speech side, cycle 71, ADR 0213; section 43); P4 mic edge cases (unplug, AirPods profile, permission revoked,
sleep/wake): talk side **N194 fixed `88c97ae`** (cycle 73, ADR 0222; section 45), N195/N196 open (Bobs, helper), N197 open (talk, needs a Mac measurement); P5 key-up to text p50/p95 budget and a bench-compare script; P6 insertion fallbacks (secure fields,
Electron, terminals); P7 idle memory/CPU and model unload without first-press cost; P8 crash safety (pill reset, echo
restart with backoff).

## 30. Cycle 57: a list of short lines, a final spoken mark, and what FluidAudio already does (ADRs 0188-0190, `3d9b8b7`)

| Run               | Exact   | Edit distance       | Regressions | False-positive hits |
| ----------------- | ------- | ------------------- | ----------- | ------------------- |
| fc370d3, 46 cases | 29 / 46 | 133 chars (9.35 %)  | -           | 0 / 59              |
| 3d9b8b7, same 46  | 32 / 46 | 98 chars (6.89 %)   | 0           | -                   |
| fc370d3, 56 cases | 29 / 56 | 236 chars (14.33 %) | -           | -                   |
| 3d9b8b7, 56 cases | 41 / 56 | 100 chars (6.07 %)  | 0           | 0 / 94              |

Regressions are counted against fc370d3's own output for every case (`polish-baseline.json` refreshed; cycle 56 had
left its six "Scratch that" entries at the older text).

- ADR 0188: a list of three or more short lines (1-3 words each, no blank line, no sentence mark but on the last)
  ends without a ".": "Groceries new line eggs new line milk" → "Groceries⏎Eggs⏎Milk" (was "…⏎Milk."). Addresses
  and sign-offs too. Two lines, a 4-word line, a paragraph break, "3 p.m.", "etc." and "?" are left as they were.
  5 positives, 9 new keep vectors.
- ADR 0189: a dictation-final lowercase "question mark" / "exclamation point" / "exclamation mark" becomes the mark
  when the word before is a plain word that can't introduce the noun, there is no article in the last 3 words, and
  (for "?") the sentence starts like a question: "Is the meeting still on question mark" → "Is the meeting still
  on?", "Great work everyone exclamation point" → "Great work everyone!". "Put a question mark", "Which key is
  question mark", "Does the title need question mark", "Shift 1 exclamation point", "The band was called Question
  Mark" stay. 8 positives, 26 new keep vectors.
- Cost (release, box CPU): final-mark step 0.0001 ms without a mark, 0.0006 ms when it fires on 490 words; the list
  check ≤ 0.004 ms. `cleanup_budget.rs` (508 words, now ending in a spoken mark) 1.50 ms median against the 2 ms
  budget (old code 1.61-2.20 ms in the same interleaved runs: box load). Nothing in capture, segmenting, the decode
  schedule or Parakeet's settings moved.
- ADR 0190 (research, no code): FluidAudio v0.17.7 gives Echo punctuation and capitals from the model already; TDT
  decoding is greedy only; the TDT settings are model-defined or long-audio-only; ITN and vocabulary boosting are
  separate opt-ins. Measured NeMo ITN (text-processing-rs v0.3.1) on the 224 texts: 5 polish cases better, 5 worse,
  **33 / 94 keep-set hits**; rejected. Boosting needs a second 97.5 MB encoder pass per segment; rejected. Proposal
  below (Jared sends it).

Next polish candidates, ranked: (1) money and percent spans only ("twenty five dollars" → "$25", "fifteen percent"
→ "15%", p17/p18), talk-side, found by talk's own patterns and converted with a closed number-word parser (or the
Apache-2.0 text-processing-rs single-expression `normalize` on that span alone), with keep vectors for "a couple of
dollars", "one percent of the time", names; (2) spoken "comma" / "period" only between two clauses where the noun is
impossible (p15), measured against k27/k28; (3) wh-questions ending in a listed verb ("What did you say question
mark"), if a closed exemption can be made unambiguous; (4) warming dictionary patterns off the press path. Backlog
P1-P8 above is still open.

### Proposal to Beve Bobs (cycle 57; Jared sends it)

From: Jared (echo cycle 57, Oct 18-19, 2026). Read against the pinned FluidAudio **v0.17.7**
(`503b4bd1bbf7220882de39fe8ae6716aae4132da`, `native/qrtz-helper/Package.swift:12-15` and `Package.resolved`),
cloned and checked out at that tag. Every claim below cites file:line in that tag. Nothing here asks for a code change
in your areas; the one optional item is a dev-only check.

#### TL;DR

- **Keep `AsrManager(config: .default, models: v2)` exactly as it is** (`QrtzEchoMac/Parakeet.swift:27-28`). There is
  no FluidAudio setting for Parakeet TDT v2 that improves the text without changing the decode or adding time.
- **Do not wire FluidAudio's ITN (`TextNormalizer`) into dictation.** I ran its engine over all 224 polish/keep/cleanup
  texts: it fixes 5 polish cases but breaks 5 others and hits **33 of the 94** false-positive vectors.
- **Do not add vocabulary boosting for v1.** For the 0.6B v2 model it needs a second, separate CTC encoder pass per
  segment (the last one on the release path) and a 97.5 MB extra model.
- Talk-side polish stays the place for this (cycle 57 landed two rules there, ADRs 0188/0189, `3d9b8b7`).

#### What Parakeet / FluidAudio does natively (v0.17.7)

1. **Punctuation and capitals come from the model itself.** `AsrManager.convertTokensToText` just joins the
   SentencePiece pieces and turns `▁` into spaces (`Sources/FluidAudio/ASR/Parakeet/SlidingWindow/TDT/AsrManager.swift:531-538`);
   `processTranscriptionResult` adds token timings and confidence, nothing else (`AsrManager+Transcription.swift:562-591`).
   There is no post-processing step and no switch for it. We already get it.
2. **Digits / ITN are not applied by `transcribe`.** No reference to `TextNormalizer` anywhere under
   `ASR/Parakeet/SlidingWindow/TDT/`. ITN is a separate, opt-in call: `TextNormalizer.normalizeSentence`
   (`Sources/FluidAudio/ITN/TextNormalizer.swift:118-129`) / `normalize(result:)` (`:154-170`). The engine is
   already linked into qrtz-helper by default: FluidAudio's target depends on the `NemoTextProcessing` binary target
   on macOS (`Package.swift:30`, `:44-49`, text-processing-rs v0.3.1 xcframework; `Package@swift-6.2.swift:35-39`
   makes it a default-on trait). qrtz-helper never calls it (no `TextNormalizer` in `native/qrtz-helper/Sources`).
   The Swift wrapper masks only 9 ambiguous words with NLTagger: period, dash, colon, pipe, slash, dot, plus, hash,
   percent (`TextNormalizer.swift:53-58`, used at `:120`). "comma", "question mark", "exclamation point" and number
   words are not masked.
3. **Decoder: greedy only.** `TdtHypothesis` is "Hypothesis for TDT greedy decoding"
   (`SlidingWindow/TDT/Decoder/TdtHypothesis.swift:1`); the TDT decoder takes the joint's argmax / top-K per step
   (`TdtDecoderV3.swift:25`, `:285-299`). Beam search exists only for the CTC path (`SlidingWindow/CTC/CtcDecoder.swift:100-122`,
   `ctcBeamSearch`), not for TDT. There is no beam option to try.
4. **TDT duration / decoder settings** (`TdtConfig.swift:1-35`): `durationBins [0,1,2,3,4]` are the model's trained
   duration head (mapping only, `TdtDurationMapping.swift:17-22`), not a tuning knob; `blankId` is rewritten to 1024
   for v2 (`TdtDecoderV2.swift:61-73`); `boundarySearchFrames` and `maxTokensPerChunk` serve the long-audio
   `ChunkProcessor` path (`AsrManager.swift:258-259`, `AsrManager+TokenProcessing.swift:188-195`). Echo never takes
   that path: segments are capped at 15 s (`QrtzEcho/Segmented.swift:54-56`, `:108` `capMs: 15_000`) and every
   decode at or under `ASRConstants.maxModelSamples = 240_000` (`Shared/ASRConstants.swift:12`) is one window with
   `isLastChunk: true` (`AsrManager+Transcription.swift:15-37`). The only settings that touch our decode are
   `maxSymbolsPerStep = 10` (`TdtDecoderV3.swift:226`, `:460`) and `consecutiveBlankLimit = 5`, which bounds the
   end-of-window flush that every Echo segment runs (`TdtDecoderV3.swift:473-486`). Changing either changes the
   decode, and so the timing and the tail words. Not proposed (see "If you ever want to try a decoder setting").
5. **Language filter, mel context, dual-decode arbitration, seam-gap repair**: v3-only or long-form-only.
   `language:` is "silently ignored for v2" (`AsrManager.swift:371-373`); the others are chunk-path options
   (`AsrTypes.swift:26-54`). Nothing to set. (Side note, harmless for us: `TdtDecoderV2.adaptConfigForV2` rebuilds
   `ASRConfig` without passing `seamGapRepair` / `seamGapRepairMinGapSeconds`, `TdtDecoderV2.swift:75-84`, so they
   fall back to defaults on v2; it matters only on the chunk path, which Echo never takes.)
6. **Vocabulary / context biasing ("hotwords").** `AsrManager` has none. `configureVocabularyBoosting` exists only on
   `SlidingWindowAsrManager` (`SlidingWindow/SlidingWindowAsrManager.swift:70-71`, `:100-110`) and the Unified
   managers (`Unified/UnifiedAsrManager.swift:205-215`). The public `VocabularyBoostingSession`
   (`CustomVocabulary/VocabularyBoostingSession.swift:11`, `rescore(text:tokenTimings:audioSamples:)` at `:98-102`)
   could rescore a segment, but: the 0.6B v2/v3 models have no CTC head (`Documentation/ASR/CustomVocabulary.md:33-36`,
   `:88-90`), so it "loads a separate 97.5MB Parakeet CTC 110M encoder" (`:135`) and runs at 25.98x RTFx instead of
   70.29x (`:145`). That is a second encoder pass on every segment, the last one after release: about 190 ms for a
   5 s final segment at their published speed (their hardware, not measured on Jared's Mac), plus ANE contention with
   the TDT encoder, a second resident model and a new download. It also rewrites ordinary words on short-term
   vocabularies unless extra floors are set (`CustomVocabulary.md:492-583`). This fails the zero-added-latency rule.
   Talk's own dictionary (`apply_dictionary`, patterns cached once per launch since ADR 0182) stays the mechanism.
7. **Models (information only).** FluidAudio's own table: v2 2.1 % WER vs v3 2.5 % on LibriSpeech test-clean,
   "Use v2 if you only need English" (`Documentation/Benchmarks.md:44-67`); the v3-family `ultra` is 2.13 % /
   3.81 % on an M5 Pro (`Benchmarks.md:69-80`, `Documentation/ASR/ParakeetUltra.md`). A model change is a new download
   and new timing; not a v1 item.

#### Evidence for "don't turn on ITN" (measured on the box, cycle 57)

I built text-processing-rs **v0.3.1** (the engine inside the xcframework) and ran `normalize_sentence`, the function
behind `nemo_normalize_sentence` (`src/ffi.rs:72-88`, `src/lib.rs:1020`), on every unique text in
`spec/vectors/talk/polish.json` (56 polish + 94 keep) and `cleanup.json` (224 texts), then the talk cleanup on its
output. NLTagger masking is Apple-only and not simulated, so the "period" hits (k27, part of k69) might not happen on
the Mac; none of the others involve a masked word.

| Pipeline                               | Polish exact | Edit distance | Polish worse | Keep-set hits |
| -------------------------------------- | ------------ | ------------- | ------------ | ------------- |
| talk cleanup (main at `3d9b8b7`)       | 41 / 56      | 100 chars     | -            | 0 / 94        |
| ITN (`normalizeSentence`) then cleanup | 39 / 56      | 60 chars      | 5            | 33 / 94       |

- Better (5): "comma ... period" → ", ... .", "three thirty p.m." → "03:30 p.m." (a leading zero, still not "3:30"),
  "twenty five dollars" → "$25", "fifteen percent" → "15 %", "qrtz dot ai slash docs" → "Thedocsare@qrtz.ai/ docs".
- Worse (5): "ten minutes" → "10 minutes" (style), "Email me at jared at gmail dot com" → "Emailme@Jared at gmail dot
  com", "One call Sam / Two book the room" → "1 call Sam / 2 book the room", "at one?" → "at 1?", and newlines in
  the input collapsed ("Hi Sam,⏎⏎The launch" lost its paragraph).
- Keep-set hits (33), e.g. "One of the best demos" → "1 of the best demos", "A table for two" → "for 2", "Add a comma
  after the name" → "Add a, after the name", "Put a question mark at the end" → "Put a? At the end", every noun
  "question mark" / "exclamation point" → "?" / "!" ("Which key is?", "Add an!", "The band was called?").
- Cost: median 0.06 ms per text, up to 1.2 ms median (1.56 ms p95) on the longest texts, box CPU, release build.
  On the release path that is added time on the last segment.

Verdict: rejected as a whole. A narrow, talk-side use of the same Apache-2.0 Rust crate (money and percent spans
only, found by talk's own patterns, single-expression `normalize` on that span) is on my candidate list; it would be
in qrtz-talk, not the helper.

#### Asks for you

1. **No change** to `ParakeetBatch` (`AsrManager(config: .default, …)`, v2, `TdtDecoderState.make` per segment).
2. **Optional, dev-only:** if you want to confirm the ITN verdict with NLTagger on a Mac, a hidden
   `qrtz-helper --itn-check <file>` that prints `TextNormalizer.shared.normalizeSentence(line)` for each line, not
   linked into any dictation path. I would score the output with the polish set. Skip it if you agree with the Linux
   result; it changes nothing in what ships.
3. **Nothing to add for hotwords** until a CTC-head model (TDT-CTC-110M) is on the table, and then only behind a bench
   that shows zero release-to-text delta.

#### Measurement plan for Jared's Mac (proves cycle 57 adds no latency and no worse transcript)

Cycle 57 changed only talk's cleanup (two string checks after release, measured 0.0006 ms and ≤ 0.004 ms on the
box; chain 1.50 ms median on 508 words, 2 ms budget enforced by `crates/qrtz-talk/tests/cleanup_budget.rs`). The
helper does not contain talk's cleanup, so `echo-bench.sh` timings (helper side) can't move from it; the steps prove
that and score the text old vs new on the same raw recognitions:

1. `cd ~/code/qrtz && git pull` (main at or after `3d9b8b7`).
2. Cleanup cost on the Mac: `cargo test --release -p qrtz-talk --test cleanup_budget -- --nocapture` (prints the
   median; fails over 2 ms). Expect well under the box's 1.50 ms.
3. Recognitions: `bash native/qrtz-helper/scripts/echo-bench.sh --quick` and, with his own voice,
   `bash native/qrtz-helper/scripts/echo-bench.sh --clips ~/qrtz-echo-bench/my-clips` (both build `origin/main`).
   Run `--quick` twice: `releaseToStopped` p50/p90 and `firstPartial` must match the last recorded cycle-56 run
   within the spread of those two runs (no helper change landed for Echo this cycle).
4. Score the same raw reports with the old cleanup:
   `git -C ~/code/qrtz worktree add --detach /tmp/qrtz-c56 fc370d3 && cd /tmp/qrtz-c56 && cargo run -q -p qrtz-talk --example echo_score -- ~/qrtz-echo-bench/results/<run>-<stamp>.json`
   then `cp <run>-<stamp>.scored.json <run>-<stamp>.c56.scored.json` (the scorer overwrites `<report>.scored.json`),
   and score again from `~/code/qrtz` with the new cleanup. Raw WER must be identical; formatted WER after `cleanup_dictation` must
   be equal or lower on **every** clip (not just the mean). Then `git -C ~/code/qrtz worktree remove /tmp/qrtz-c56`.
5. In real use: the timing log's cleanup stage stays well under 1 ms; `release→inserted` unchanged beyond noise.
6. If any clip gets worse: revert `3d9b8b7` (talk-only commit).

##### If you ever want to try a decoder setting (not proposed now)

Same harness, one setting per run (for example `consecutiveBlankLimit` 5 → 8, or `maxSymbolsPerStep`), on the same
build: `releaseToStopped` p95 delta must be ≤ 0 within the baseline spread, and per-clip raw WER and tail-word loss
(`tailWordsLost`) no worse on any clip, including `--clips` with short last words. Anything else stays at default.

## 31. Cycle 58: a filler's comma, and spoken amounts as figures (ADRs 0192, 0193, `5625562`)

| Run               | Exact   | Edit distance      | Regressions | False-positive hits |
| ----------------- | ------- | ------------------ | ----------- | ------------------- |
| 85bc513, 56 cases | 41 / 56 | 100 chars (6.07 %) | -           | 0 / 94              |
| 5625562, same 56  | 48 / 56 | 50 chars (3.04 %)  | 0           | -                   |
| 85bc513, 68 cases | 41 / 68 | 195 chars (9.88 %) | -           | -                   |
| 5625562, 68 cases | 60 / 68 | 50 chars (2.53 %)  | 0           | 0 / 143             |

Regressions are counted against 85bc513's own output for every case (`polish-baseline.json` refreshed); 19 cases
better, none worse.

- ADR 0192: a filler set off by commas takes the comma before it along, only after a word no comma can follow: "Can
  you, um, send me the deck?" → "Can you send me the deck?" (was "Can you, send …"), "The demo is, um, basically done."
  → "The demo is basically done.", "talk to the, um, design team" → "talk to the design team", "and, uh, we think" →
  "and we think". A comma the sentence needs stays: "So, uh, the build", "If we can, um, we ship", "Yes we can, um,
  Sam", "The thing is, um, we're late", "Plan A, um, then". 10 positives, 23 new keep vectors.
- ADR 0193: spoken amounts in their one-reading forms: "twenty five dollars" → "$25", "two thousand five hundred
  dollars" → "$2,500", "fifteen percent" / "15 percent" → "15%", "three thirty p.m." → "3:30 p.m.", "ten oh five a.m."
  → "10:05 a.m.". Ranges, decimals in words, cents, scales, "a hundred", "fifteen hundred", sentence starts, the
  singular "dollar" and digits before "dollars" stay as spoken. 9 positives, 26 new keep vectors.
- Cost (release, box CPU): each rule ~0.03 ms on 495 words with ~40 triggers, ~1-3 µs on one sentence; whole chain on
  one sentence with both rules 0.045 ms. `cleanup_budget.rs` (495 words, both rules' triggers added), interleaved 3×:
  old 1.558 / 1.838 / 1.538 ms median, new 1.616 / 1.626 / 1.621 ms; 2 ms budget holds. Nothing in capture,
  segmenting, the decode schedule or Parakeet's settings moved; the cleanup runs once on the final text
  (`service.rs:1437`, `:1444`, `:1541`), never on partials.
- Path review: no capture, VAD, model config, drain, insert, pill or timing change since 85bc513 (origin moved only by
  ctrl run 49, `5671bb3`, ADR 0191). Findings list unchanged; backlog P1-P8 above still open (polish took the cycle).

Still not exact (8 of 68): p10 "So we, we can" (repeat across a comma), p15 spoken "comma"/"period", p19/p20 email and
URL, p38/p39 an unpunctuated question, p41 a numbered list with a 4-word line, p49 commas before "new line" in a list.

Next polish candidates, ranked: (1) "?" for a dictation that ends with no mark at all and starts like a yes/no
question (auxiliary + subject pronoun: "Do you want to grab lunch", p39), measured against declaratives that start
with an auxiliary; (2) ranges as figures when both ends are amounts ("ten to fifteen percent" → "10-15%"?) only if a
single unambiguous form exists; (3) the comma before a spoken "new line" when every line of a short list has one
(p49), against salutations and verse; (4) warming dictionary patterns off the press path.

### Measurement plan for Jared's Mac (cycle 58; proves no added latency and no worse transcript)

Talk-only change (`5625562`); the helper does not run talk's cleanup, so `echo-bench.sh` timings can't move from it.

1. `cd ~/code/qrtz && git pull` (main at or after `5625562`).
2. `cargo test --release -p qrtz-talk --test cleanup_budget -- --nocapture`: median under 2 ms (box 1.62 ms).
3. `bash native/qrtz-helper/scripts/echo-bench.sh --quick` twice: `releaseToStopped` p50/p90 and `firstPartial` within
   the spread of the last recorded run.
4. Score the same raw reports with the old cleanup: `git -C ~/code/qrtz worktree add --detach /tmp/qrtz-c57 85bc513 &&
   cd /tmp/qrtz-c57 && cargo run -q -p qrtz-talk --example echo_score -- ~/qrtz-echo-bench/results/<run>-<stamp>.json`,
   keep a copy of `<report>.scored.json`, then score again from `~/code/qrtz`. Raw WER identical; formatted WER equal
   or lower on every clip. `git -C ~/code/qrtz worktree remove /tmp/qrtz-c57`.
5. If any clip gets worse: revert `5625562` (talk-only commit).

## 32. Cycle 59: an unmarked question ends with "?" (ADR 0196, `f40d440`)

| Run               | Exact   | Edit distance     | Regressions | False-positive hits |
| ----------------- | ------- | ----------------- | ----------- | ------------------- |
| 66b4475, 68 cases | 60 / 68 | 50 chars (2.53 %) | -           | 0 / 143             |
| f40d440, same 68  | 62 / 68 | 48 chars (2.43 %) | 0           | -                   |
| 66b4475, 79 cases | 60 / 79 | 61 chars (2.58 %) | -           | -                   |
| f40d440, 79 cases | 73 / 79 | 48 chars (2.03 %) | 0           | 0 / 180             |

Regressions are counted against 66b4475's own output for every case (`polish-baseline.json` then refreshed to
`f40d440`); 13 cases better, none worse.

- ADR 0196: where the cleanup would add "." to a dictation Parakeet left unmarked, it adds "?" when the last
  sentence can only be a question: "Do you want to grab lunch" → "Do you want to grab lunch?", "what time does the
  store close" → "What time does the store close?", "is there anything else you need from me" → "Is there anything
  else you need from me?". Only a yes/no opening (auxiliary + an agreeing pronoun) or a wh-opening (wh-word,
  auxiliary, pronoun or determiner), 3-12 words, no comma or apostrophe, no second clause after the opening. A mark
  the recogniser wrote, a spoken "question mark" (ADR 0189) and short dictations are untouched. Commands ("Do the
  dishes"), dropped subjects ("Was the best day ever"), conditional and exclamatory inversions ("Should you need
  anything", "Was I ever glad") keep their ".". 13 positives, 37 new keep vectors.
- Cost (release, box CPU): the check reads only the last sentence once; one sentence 0.016 ms whole chain before and
  after. `cleanup_budget.rs` interleaved 3×: old 1.845 / 1.618 / 1.604 ms median, new 1.613 / 1.600 / 1.617 ms; its
  new part (504 words ending in an unmarked question, so the check runs and fires) 1.70 ms. Budget 2 ms holds, but
  the chain now sits at ~1.6-1.7 ms on the box: the next rule must pay for itself or come with a speed-up.
- Path review: no capture, VAD, model config, drain, insert, pill or timing change since 66b4475 (origin moved only by
  ctrl run 50, `f00239b`, ADR 0194). Findings list unchanged; backlog P1-P8 still open (polish took the cycle).

Still not exact (6 of 79): p10 "So we, we can" (repeat across a comma), p15 spoken "comma"/"period", p19/p20 email and
URL, p41 a numbered list with a 4-word line, p49 commas before "new line" in a list.

Next polish candidates, ranked: (1) the comma before a spoken "new line" when every line of a short list has one
(p49), against salutations ("Dear Sam, new line") and verse; (2) a discourse opener before a question ("So do you
want", "Okay can you") with its own false-positive set ("So do it now"); (3) a cheaper chain (warm dictionary
patterns off the press path, one pass for the word lists) to win back budget headroom before more rules.

### Measurement plan for Jared's Mac (cycle 59; proves no added latency and no worse transcript)

Talk-only change (`f40d440`); the helper does not run talk's cleanup, so `echo-bench.sh` timings can't move from it.

1. `cd ~/code/qrtz && git pull` (main at or after `f40d440`).
2. `cargo test --release -p qrtz-talk --test cleanup_budget -- --nocapture`: both medians under 2 ms (box 1.6-1.7 ms).
3. `bash native/qrtz-helper/scripts/echo-bench.sh --quick` twice: `releaseToStopped` p50/p90 and `firstPartial` within
   the spread of the last recorded run.
4. Score the same raw reports with the old cleanup: `git -C ~/code/qrtz worktree add --detach /tmp/qrtz-c58 66b4475 &&
   cd /tmp/qrtz-c58 && cargo run -q -p qrtz-talk --example echo_score -- ~/qrtz-echo-bench/results/<run>-<stamp>.json`,
   keep a copy of `<report>.scored.json`, then score again from `~/code/qrtz`. Raw WER identical; formatted WER equal
   or lower on every clip. `git -C ~/code/qrtz worktree remove /tmp/qrtz-c58`.
5. If any clip gets worse: revert `f40d440` (talk-only commit).

## 33. Cycles 60-61: the cleanup chain, same text, ~12x faster (ADR 0199, `2bfa59c`)

No text change, so no score change: 73 / 79 exact, 48 chars (2.03 %), **regressions 0**, keep set 180 with **0 hits**.
What changed is cost: section 32 left ~0.3 ms of headroom in the 2 ms budget; the steps whose patterns had a
look-around or a backreference (spoken layout commands, fillers, repeats, the continued dot, sentence starts, the lone
"i", the dictionary replace) now run as hand-written scans of the same patterns.

| Release, box CPU, median          | d409ede (old) | 2bfa59c (new) |
| --------------------------------- | ------------- | ------------- |
| 495-word dictation, whole chain   | 1.48-1.50 ms  | 0.11-0.20 ms  |
| 504 words ending in a question    | 1.53-1.85 ms  | 0.115 ms      |
| One sentence with a one-term dict | 0.030 ms      | 0.0036 ms     |

- Proof of same text: `tests/hardening60.rs` runs the frozen cycle-59 chain (`tests/oracle/cleanup_c59.rs`) beside the
  new one on every cleanup.json / polish.json text with four dictionaries, plus 60,000 generated texts (3,000,000 in
  release twice in cycle 60, identical). It fails without the fix (≤ 0.5 ms ceiling on the 495-word dictation).
- Measurement caveat found on the way: two worktrees sharing `CARGO_TARGET_DIR` build the same test-binary names, so
  "old vs new interleaved" numbers in sections 31-32 may have compared one binary. From now on, bench binaries are
  built one after the other and copied to /tmp before running.
- Path review: no capture, VAD, model config, drain, insert, pill or timing change since d409ede (origin moved only by
  ctrl runs 52-53, ADRs 0197-0198). Findings and backlog P1-P8 unchanged; next cycle takes polish candidate (1) or (2)
  from section 32, now with ~1.8 ms of headroom, or P1 if no polish rule clears the bar.

### Measurement plan for Jared's Mac (cycle 61)

Talk-only change; the helper doesn't run talk's cleanup, so `echo-bench.sh` timings can't move from it.

1. `cd ~/code/qrtz && git pull` (main at or after `2bfa59c`).
2. `cargo test --release -p qrtz-talk --test cleanup_budget --test hardening60 -- --nocapture`: medians well under
   0.5 ms, and the oracle test passes (same text as cycle 59 on every vector).
3. Optional: hold fn, dictate a long paragraph with "um", a repeated word and "new line"; the text inserted reads the
   same as before, and release-to-text feels unchanged.
4. If anything reads differently: revert `2bfa59c` (talk-only commit).

## 34. Cycle 62: a dictated list drops the comma before each "new line" (ADR 0201, `d68b415`)

| Run               | Exact   | Edit distance     | Regressions | False-positive hits |
| ----------------- | ------- | ----------------- | ----------- | ------------------- |
| fc07f63, 79 cases | 73 / 79 | 48 chars (2.03 %) | -           | 0 / 180             |
| d68b415, same 79  | 74 / 79 | 46 chars (1.94 %) | 0           | -                   |
| fc07f63, 83 cases | 73 / 83 | 59 chars (2.38 %) | -           | 0 / 192             |
| d68b415, 83 cases | 78 / 83 | 46 chars (1.85 %) | 0           | 0 / 192             |

Regressions are counted against `fc07f63`'s own output for every case (`polish-baseline.json` then refreshed to
`d68b415`); 5 cases better (p49, p80-p83), none worse.

- ADR 0201: "Groceries, new line, eggs, new line, milk." → "Groceries⏎Eggs⏎Milk" (was "Groceries,⏎Eggs,⏎Milk").
  Only when the whole text is a list: 3+ lines, no blank line, a comma on every line but the last, every line 1-2
  words of ASCII letters, every line after the first lowercase as Parakeet wrote it, no greeting/sign-off word, and a
  recogniser that writes capitals. Greetings and sign-offs ("Hi team,⏎Thanks,⏎Jared"), names and addresses, 3-word
  verse, digits, apostrophes, inner commas and partial lists keep every comma. 5 positives, 12 new keep vectors,
  13 new cleanup.json vectors (Rust and TS same text).
- Cost (release, box CPU; old and new `cleanup_budget` binaries built one after the other, copied to /tmp, run
  interleaved 3×): 495 words old 0.111 / 0.122 / 0.121 ms median, new 0.111 / 0.119 / 0.111 ms; 504 words ending in a
  question old 0.115 / 0.123 / 0.115 ms, new 0.120 / 0.123 / 0.115 ms. hardening60 ceiling (≤ 0.5 ms): 0.131 ms.
  No added latency: the step returns at once without a ",⏎" in the text.
- `tests/hardening60.rs`'s oracle (the frozen cycle-59 chain) now allows exactly this difference and nothing else.
- Path review: no capture, VAD, model config, drain, insert, pill or timing-log change since `267c826` (origin moved
  only by ctrl run 54, `fc07f63`, ADR 0200: qrtz-ctrl, the ctrl helper sources and ctrl mocks/e2e). Findings list
  unchanged; backlog P1-P8 still open (polish took the cycle; P1 the `local_models` timeout is next when no polish
  rule clears the bar).

Still not exact (5 of 83): p10 "So we, we can" (repeat across a comma), p15 spoken "comma"/"period", p19/p20 email
and URL, p41 a numbered list with a 4-word line.

Next polish candidates, ranked: (1) a discourse opener before a question ("So do you want", "Okay can you") getting
"?" (extend ADR 0196), with its own false-positive set ("So do it now", "Okay do the dishes"); (2) spoken "comma" /
"period" between list items (p15) only in an unambiguous enumeration ("X comma Y comma and Z period"), against "a comma
splice", "the period of"; (3) p10's repeat across a comma for "we" / "you" only before a modal ("we, we can").

### Measurement plan for Jared's Mac (cycle 62; proves no added latency and no worse transcript)

Talk-only change (`d68b415`); the helper does not run talk's cleanup, so `echo-bench.sh` timings can't move from it.

1. `cd ~/code/qrtz && git pull` (main at or after `d68b415`).
2. `cargo test --release -p qrtz-talk --test cleanup_budget --test hardening60 --test hardening62 --test polish_set --
   --nocapture`: budget medians well under 0.5 ms (box 0.11-0.12 ms), polish 78/83 exact, regressions 0, all green.
3. Score the same raw reports with the old cleanup: `git -C ~/code/qrtz worktree add --detach /tmp/qrtz-c61 fc07f63 &&
   cd /tmp/qrtz-c61 && cargo run -q -p qrtz-talk --example echo_score -- ~/qrtz-echo-bench/results/<run>-<stamp>.json`,
   keep a copy of `<report>.scored.json`, then score again from `~/code/qrtz`. Raw WER identical; formatted WER equal
   or lower on every clip. `git -C ~/code/qrtz worktree remove /tmp/qrtz-c61`.
4. Hold fn and say "Groceries, new line, eggs, new line, milk": it types three lines with no commas. Then "Hi team, new
   line, thanks, new line, Jared": the commas stay.
5. If anything reads worse: revert `d68b415` (talk-only commit).

## 35. Cycle 63: a question after a discourse opener ends with "?" (ADR 0203, `e686c05`)

| Run               | Exact   | Edit distance     | Regressions | False-positive hits |
| ----------------- | ------- | ----------------- | ----------- | ------------------- |
| 2a414ac, 83 cases | 78 / 83 | 46 chars (1.85 %) | -           | 0 / 192             |
| e686c05, same 83  | 78 / 83 | 46 chars (1.85 %) | 0           | -                   |
| 2a414ac, 91 cases | 78 / 91 | 56 chars (2.02 %) | -           | -                   |
| e686c05, 91 cases | 84 / 91 | 48 chars (1.73 %) | 0           | 0 / 219             |

Regressions are counted against `2a414ac`'s own output for every case (`polish-baseline.json` then refreshed to
`e686c05`); 8 cases better (p84-p91), none worse.

- ADR 0203: "So do you want to grab lunch" → "So do you want to grab lunch?" (was "."). Also "Okay, can you send me the
  file", "Hey are you free tomorrow afternoon", "And did you hear back from them", "So what time does the store close".
  Openers so/okay/ok/hey/and/but/also/alright, with or without their own comma; the rest must pass every ADR 0196
  check. A bare "So" needs do/does/did/can/could/will/would and a plain verb after the subject ("So did we last year",
  "So do I at work" keep "."); "and"/"but" need "you" ("And was he mad" keeps "."); "well", "oh", "now", "boy", "man"
  are not openers ("Well do I remember that day", "Oh do I love that song"). 8 positives, 27 new keep vectors, 24 new
  cleanup.json vectors (Rust and TS same text).
- Cost (release, box CPU; old and new `cleanup_budget` binaries interleaved 15×, median of medians): 495 words 0.120 →
  0.114 ms; 504 words ending in a question 0.120 → 0.125 ms (min 0.114 vs 0.115). Noise: the step is one word
  comparison on the last sentence of the final text.
- Path review: no capture, VAD, model config, drain, insert, pill or timing-log change since `4fa7603` (origin moved by
  Bobs's echo launch cdhash check `53b260c`, mock config `26d0f37`, qrtz-publish test `1ad50a7`, ctrl run 55 `ea16cf0`,
  oxfmt `2a414ac`). Findings list unchanged; backlog P1-P8 still open (P1 next when no polish rule clears the bar).

Still not exact (7 of 91): p10 "So we, we can" (repeat across a comma), p15 spoken "comma"/"period", p19/p20 email
and URL, p41 a numbered list with a 4-word line, p86/p90 the comma after "Hey"/"OK" before a question.

Next polish candidates, ranked: (1) spoken "comma" / "period" between list items (p15) only in an unambiguous
enumeration ("X comma Y comma and Z period"), against "a comma splice", "the period of"; (2) p10's repeat across a
comma for "we" / "you" only before a modal ("we, we can"); (3) the comma after "Hey" / "Okay" / "OK" only when an ADR
0203 question follows (p86, p90), against "Hey Sam", "Okay so".

### Measurement plan for Jared's Mac (cycle 63; proves no added latency and no worse transcript)

Talk-only change (`e686c05`); the helper does not run talk's cleanup, so `echo-bench.sh` timings can't move from it.

1. `cd ~/code/qrtz && git pull` (main at or after `e686c05`).
2. `cargo test --release -p qrtz-talk --test cleanup_budget --test hardening60 --test hardening63 --test polish_set --
   --nocapture`: budget medians well under 0.5 ms (box 0.11-0.13 ms), polish 84/91 exact, regressions 0, all green.
3. Score the same raw reports with the old cleanup: `git -C ~/code/qrtz worktree add --detach /tmp/qrtz-c62 2a414ac &&
   cd /tmp/qrtz-c62 && cargo run -q -p qrtz-talk --example echo_score -- ~/qrtz-echo-bench/results/<run>-<stamp>.json`,
   keep a copy of `<report>.scored.json`, then score again from `~/code/qrtz`. Raw WER identical; formatted WER equal
   or lower on every clip. `git -C ~/code/qrtz worktree remove /tmp/qrtz-c62`.
4. Hold fn and say "So do you want to grab lunch" (no pause at the end): it types "So do you want to grab lunch?".
   Then "I went to Paris last summer. So did we last year": it ends with ".".
5. If anything reads worse: revert `e686c05` (talk-only commit).

## 36. Cycle 64: an interjection before a question gets its comma (ADR 0204, `e5568d2`)

| Run               | Exact   | Edit distance     | Regressions | False-positive hits |
| ----------------- | ------- | ----------------- | ----------- | ------------------- |
| 41e066f, 91 cases | 84 / 91 | 48 chars (1.73 %) | -           | 0 / 219             |
| e5568d2, same 91  | 86 / 91 | 46 chars (1.66 %) | 0           | -                   |
| 41e066f, 94 cases | 84 / 94 | 51 chars (1.78 %) | -           | -                   |
| e5568d2, 94 cases | 89 / 94 | 46 chars (1.61 %) | 0           | 0 / 232             |

Regressions are counted against `41e066f`'s own output for every case (`polish-baseline.json` then refreshed to
`e5568d2` in the docs commit); 5 cases better (p86, p90, p92-p94), none worse.

- ADR 0204: "Hey are you free tomorrow afternoon" → "Hey, are you free tomorrow afternoon?" (was "Hey are you free
  tomorrow afternoon?"); "OK can we move the meeting to Thursday" → "OK, can we move the meeting to Thursday?". Only
  hey/okay/ok/alright, only where ADR 0203 adds "?", only with no comma of their own. 5 polish positives, 13 new keep
  vectors, 23 new cleanup.json vectors (Rust and TS same text).
- Cost (release, box CPU; old and new `cleanup_budget` binaries built one after the other, copied to /tmp, interleaved
  15×, median of medians): 495 words 0.122 → 0.119 ms; 504 words ending in a question 0.123 → 0.121 ms (noise).
- Path review: no capture, VAD, model config, drain, insert, pill or timing-log change since `41e066f` (origin moved
  only by the echo speed benchmark kit `00c6cc5`, which measures key-up to inserted text on the Mac and is the tool to
  use for backlog P5). Findings list unchanged; backlog P1-P8 still open.
- Ranked findings, refreshed (file:line on `e5568d2`): cleanup.rs `rule_cleanup` (~l.1720) runs once on the final
  text after release, `terminal_punctuation` (~l.1493) → `unmarked_question` (~l.1434) → `opener_comma` (~l.1525); the
  TS twin is cleanup.ts `terminalPunctuation` / `unmarkedQuestion` (~l.324) / `openerComma` (~l.409).

Still not exact (5 of 94): p10 "So we, we can" (repeat across a comma), p15 spoken "comma"/"period", p19/p20 email
and URL, p41 a numbered list with a 4-word line.

Next polish candidates, ranked: (1) p10's repeat across a comma for "we"/"they" only before a modal and only when the
word before the first "we" is a clause opener (so/and/but/then/because/sentence start), refusing "So do we, we just",
"Neither have we, we", "than we, we"; collect 20+ refusals before landing. (2) p15 spoken "comma"/"period" only after
measuring on the Mac how Parakeet writes a spoken "comma" (step 5 below). (3) a vocative name after an interjection
before an ADR 0203 question ("Hey Sam are you free" → "Hey Sam, are you free?"), only for a dictionary name.

### Measurement plan for Jared's Mac (cycle 64; proves no added latency and no worse transcript)

Talk-only change (`e5568d2`); the helper does not run talk's cleanup, so `echo-bench.sh` timings can't move from it.

1. `cd ~/code/qrtz && git pull` (main at or after `e5568d2`).
2. `cargo test --release -p qrtz-talk --test cleanup_budget --test hardening60 --test hardening63 --test hardening64
   --test polish_set -- --nocapture`: budget medians well under 0.5 ms (box 0.11-0.12 ms), polish 89/94 exact,
   regressions 0, all green.
3. Score the same raw reports with the old cleanup: `git -C ~/code/qrtz worktree add --detach /tmp/qrtz-c63 41e066f &&
   cd /tmp/qrtz-c63 && cargo run -q -p qrtz-talk --example echo_score -- ~/qrtz-echo-bench/results/<run>-<stamp>.json`,
   keep a copy of `<report>.scored.json`, then score again from `~/code/qrtz`. Raw WER identical; formatted WER equal
   or lower on every clip. `git -C ~/code/qrtz worktree remove /tmp/qrtz-c63`.
4. Hold fn and say "Hey are you free tomorrow afternoon" (no pause at the end): it types "Hey, are you free tomorrow
   afternoon?". Then "Hey Sam are you free tomorrow": it types "Hey Sam are you free tomorrow." (no comma).
5. For the next cycle's p15 decision: hold fn and say "Bring the charger comma the cable comma and the adapter period";
   note whether the raw text (Echo history, raw view) has the word "comma" or a ",".
6. If anything reads worse: revert `e5568d2` (talk-only commit).

## 37. Cycle 65: a "we, we" stutter before a modal collapses (ADR 0205, `dd70d91`)

| Run               | Exact   | Edit distance     | Regressions | False-positive hits |
| ----------------- | ------- | ----------------- | ----------- | ------------------- |
| 9681f26, 94 cases | 89 / 94 | 46 chars (1.61 %) | -           | 0 / 232             |
| dd70d91, same 94  | 90 / 94 | 42 chars (1.47 %) | 0           | -                   |
| 9681f26, 97 cases | 89 / 97 | 60 chars (2.00 %) | -           | -                   |
| dd70d91, 97 cases | 93 / 97 | 42 chars (1.40 %) | 0           | 0 / 245             |

Regressions are counted against `9681f26`'s own output for every case (`polish-baseline.json` then refreshed to
`dd70d91` in the docs commit); 4 cases better (p10, p95-p97), none worse.

- ADR 0205: "So we, we can ship it tomorrow." → "So we can ship it tomorrow."; also "We, we should probably push the
  launch to next week.", "But they, they will call you back tomorrow.". Only "we"/"they", only exactly ", " between,
  only before a plain modal, only when the first pronoun opens its sentence (alone or after so/and/but/then/because).
  "Neither have we, we were busy", "So do we, we just", "You and we, we can", "So we, we went", "So we, we can't" keep
  both words. 3 polish positives, 13 new keep vectors, 39 new cleanup.json vectors (Rust and TS same text).
- Cost (release, box CPU; old and new `cleanup_budget` binaries interleaved 15×, median of medians): 495 words 0.119 →
  0.123 ms (min 0.110 → 0.109); 504 words ending in a question 0.123 → 0.124 ms. Noise.
- Path review: no capture, VAD, model config, drain, insert, pill or timing-log change since `9681f26` (origin moved
  only by the benchmark method review `b458c80`). Findings list unchanged; backlog P1-P8 still open.

Still not exact (4 of 97): p15 spoken "comma"/"period", p19/p20 email and URL, p41 a numbered list with a 4-word line.

Next polish candidates, ranked: (1) p15 spoken "comma"/"period", only after the Mac check in section 36 step 5 shows
whether Parakeet writes the word or the mark. (2) a vocative dictionary name after an interjection before an ADR 0203
question ("Hey Sam are you free" → "Hey Sam, are you free?"). (3) negated modals in ADR 0205 ("So we, we can't"),
with their own refusals. When none clears the bar, backlog P1 (the local_models test timeout).

### Measurement plan for Jared's Mac (cycle 65; proves no added latency and no worse transcript)

Talk-only change (`dd70d91`); the helper does not run talk's cleanup, so `echo-bench.sh` timings can't move from it.

1. `cd ~/code/qrtz && git pull` (main at or after `dd70d91`).
2. `cargo test --release -p qrtz-talk --test cleanup_budget --test hardening60 --test hardening65 --test polish_set --
   --nocapture`: budget medians well under 0.5 ms (box 0.11-0.13 ms), polish 93/97 exact, regressions 0, all green.
3. Score the same raw reports with the old cleanup: `git -C ~/code/qrtz worktree add --detach /tmp/qrtz-c64 9681f26 &&
   cd /tmp/qrtz-c64 && cargo run -q -p qrtz-talk --example echo_score -- ~/qrtz-echo-bench/results/<run>-<stamp>.json`,
   keep a copy of `<report>.scored.json`, then score again from `~/code/qrtz`. Raw WER identical; formatted WER equal
   or lower on every clip. `git -C ~/code/qrtz worktree remove /tmp/qrtz-c64`.
4. Hold fn and say "So we, we can ship it tomorrow" (stumble on "we" on purpose): it types "So we can ship it
   tomorrow.". Then "Neither have we, we were busy": both words stay.
5. If anything reads worse: revert `dd70d91` (talk-only commit).

## 38. Cycle 66: a dictionary name after "Hey" before a question gets its comma (ADR 0206, `965b990`)

| Run                | Exact    | Edit distance     | Regressions | False-positive hits |
| ------------------ | -------- | ----------------- | ----------- | ------------------- |
| 26e5aaa, 97 cases  | 93 / 97  | 42 chars (1.40 %) | -           | 0 / 245             |
| 965b990, same 97   | 93 / 97  | 42 chars (1.40 %) | 0           | -                   |
| 26e5aaa, 101 cases | 93 / 101 | 50 chars (1.58 %) | -           | -                   |
| 965b990, 101 cases | 97 / 101 | 42 chars (1.33 %) | 0           | 0 / 262             |

Regressions are counted against `26e5aaa`'s own output for every case, with "Priya" already in the polish dictionary
(it changes no old case); `polish-baseline.json` refreshed to `965b990` in the docs commit. 4 cases better (p98-p101),
none worse.

- ADR 0206: "Hey Jared are you free tomorrow afternoon" → "Hey Jared, are you free tomorrow afternoon?"; also "hey
  jared did you get my email" → "Hey Jared, did you get my email?", "I just landed. Hey pria where are you parked" →
  "I just landed. Hey Priya, where are you parked?". Only "hey", only a dictionary term shaped like a name (one ASCII
  word, capital first, not a word the question test reads), only where "Hey" + the words after the name is an ADR
  0203 question with I/you/we/they/he/she as subject (or a wh-word other than "who"), "did you" only before one of the
  25 plain verbs. "Hey Sam are you free" (Sam not in the dictionary), "Hey Jared has it covered", "Hey Jared is that
  guy from the meetup", "Hey Jared did you a favor last week", "Hey Jared who is the new designer", "Okay Jared can
  you send it over" stay. 4 polish positives, 17 new keep vectors (the keep set now reads `"dict": true` to run a
  vector with the polish dictionary), 32 new cleanup.json vectors (Rust and TS same text).
- Cost (release, box CPU; old and new `cleanup_budget` binaries built one after the other, copied to /tmp,
  interleaved 15×, median of medians): 495 words 0.123 → 0.116 ms (min 0.110 → 0.107); 504 words ending in a question
  0.127 → 0.126 ms; 504 words ending in "Hey Postgres can you take the backup tonight" (new third timed text; the rule
  runs in full and fires) 0.135 → 0.131 ms. Noise; ceiling 2 ms.
- Path review: origin/main had not moved since `26e5aaa`; no capture, VAD, model config, drain, insert, pill or
  timing-log change (talk's cleanup is still the only post-processing, cleanup.rs `rule_cleanup` ~l.1885, run once on
  the final text). Findings list unchanged; backlog P1-P8 still open.

Still not exact (4 of 101): p15 spoken "comma"/"period", p19/p20 email and URL, p41 a numbered list with a 4-word line.

Rejected this cycle (details in ADR 0206): any capitalised word as a name without the dictionary; "okay"/"ok"/
"alright" before a name; "it"/"this"/"that"/"there" after the auxiliary; "did you" before any word; "who"; two-word,
non-ASCII and lowercase terms; a comma already after the name; ADR 0205's negated modals (deferred, not refused).

Next polish candidates, ranked: (1) p15 spoken "comma"/"period", only after the Mac check in section 36 step 5. (2)
negated modals in ADR 0205 ("So we, we can't make it" → "So we can't make it"; `can't`/`couldn't`/`won't`/
`wouldn't`/`shouldn't`/`cannot`, ASCII and curly apostrophe), moving k-vectors and hardening65's refusal of "So we,
we can't" to positives with new refusals. (3) "Hey Jared did you <verb>" for verbs outside the 25 (needs a closed verb
list with its own refusals). When none clears the bar, backlog P1 (the local_models test timeout).

### Measurement plan for Jared's Mac (cycle 66; proves no added latency and no worse transcript)

Talk-only change (`965b990`); the helper does not run talk's cleanup, so `echo-bench.sh` timings can't move from it.

1. `cd ~/code/qrtz && git pull` (main at or after `965b990`).
2. `cargo test --release -p qrtz-talk --test cleanup_budget --test hardening60 --test hardening66 --test polish_set --
   --nocapture`: three budget medians well under 0.5 ms (box 0.12-0.13 ms), polish 97/101 exact, regressions 0, all
   green.
3. Score the same raw reports with the old cleanup: `git -C ~/code/qrtz worktree add --detach /tmp/qrtz-c65 26e5aaa &&
   cd /tmp/qrtz-c65 && cargo run -q -p qrtz-talk --example echo_score -- ~/qrtz-echo-bench/results/<run>-<stamp>.json`,
   keep a copy of `<report>.scored.json`, then score again from `~/code/qrtz`. Raw WER identical; formatted WER equal
   or lower on every clip. `git -C ~/code/qrtz worktree remove /tmp/qrtz-c65`.
4. In Echo's dictionary add a name you dictate often (for example "Priya"). Hold fn and say "Hey Priya are you free
   tomorrow": it types "Hey Priya, are you free tomorrow?". Then "Hey Priya has it covered": it types "Hey Priya has
   it covered." (no comma, "."). A name not in the dictionary ("Hey Sam are you free tomorrow") types as before.
5. If anything reads worse: revert `965b990` (talk-only commit; its vectors and tests go with it).

## 39. Cycle 67: a subject stutter before a negated modal collapses (ADR 0207, `dd2a416`)

| Run                | Exact     | Edit distance     | Regressions | False-positive hits |
| ------------------ | --------- | ----------------- | ----------- | ------------------- |
| 08dae4e, 101 cases | 97 / 101  | 42 chars (1.33 %) | -           | 0 / 262             |
| dd2a416, same 101  | 97 / 101  | 42 chars (1.33 %) | 0           | -                   |
| 08dae4e, 105 cases | 97 / 105  | 60 chars (1.81 %) | -           | -                   |
| dd2a416, 105 cases | 101 / 105 | 42 chars (1.27 %) | 0           | 0 / 275             |

Regressions are counted against `08dae4e`'s own output for every case (`polish-baseline.json` then refreshed to
`dd2a416` in the docs commit); 4 cases better (p102-p105), none worse.

- ADR 0207: "So we, we can't make it to the dinner." → "So we can't make it to the dinner."; also "So we, we cannot
  join the call this afternoon.", "But they, they won't be ready until Friday.", "We, we shouldn't ship without the
  tests.". Same sentence-start / opener / comma / lowercase-second-pronoun guards as ADR 0205; modal list gains
  can't/couldn't/won't/wouldn't/shouldn't/mustn't/mightn't/mayn't/cannot (ASCII and curly apostrophe). "So we, we
  candidly told them", "So we, we cant make it", "Neither have we, we can't wait", "You and we, we can't share" keep
  both words. 4 polish positives (k241 moved off the keep set), 14 new keep vectors, 35 new cleanup.json vectors plus
  2 flipped (Rust and TS same text).
- Cost (release, box CPU; old and new `cleanup_budget` binaries built one after the other, copied to /tmp,
  interleaved 15×, median of medians): 495 words 0.110 → 0.126 ms; 504 words ending in a question 0.120 → 0.130 ms;
  504 words ending in a vocative question 0.123 → 0.136 ms. Noise; ceiling 2 ms.
- Path review: origin/main had not moved since `08dae4e`; no capture, VAD, model config, drain, insert, pill or
  timing-log change (talk's cleanup is still the only post-processing, cleanup.rs `rule_cleanup`, run once on the
  final text). Findings list unchanged; backlog P1-P8 still open.

Still not exact (4 of 105): p15 spoken "comma"/"period", p19/p20 email and URL, p41 a numbered list with a 4-word line.

Rejected this cycle (details in ADR 0207): `shan't`; "cant" without an apostrophe; capitalised modals; reopening
ADR 0205's rejected openings (you/he/it, any verb, mid-sentence openers, triples, okay/well/hey); p15 (still waiting
on the Mac check).

Next polish candidates, ranked: (1) p15 spoken "comma"/"period", only after the Mac check in section 36 step 5. (2)
"Hey Jared did you <verb>" for verbs outside the 25 (needs a closed verb list with its own refusals). When none
clears the bar, backlog P1 (the local_models test timeout).

### Measurement plan for Jared's Mac (cycle 67; proves no added latency and no worse transcript)

Talk-only change (`dd2a416`); the helper does not run talk's cleanup, so `echo-bench.sh` timings can't move from it.

1. `cd ~/code/qrtz && git pull` (main at or after `dd2a416`).
2. `cargo test --release -p qrtz-talk --test cleanup_budget --test hardening60 --test hardening67 --test polish_set --
   --nocapture`: three budget medians well under 0.5 ms (box 0.11-0.14 ms), polish 101/105 exact, regressions 0, all
   green.
3. Score the same raw reports with the old cleanup: `git -C ~/code/qrtz worktree add --detach /tmp/qrtz-c66 08dae4e &&
   cd /tmp/qrtz-c66 && cargo run -q -p qrtz-talk --example echo_score -- ~/qrtz-echo-bench/results/<run>-<stamp>.json`,
   keep a copy of `<report>.scored.json`, then score again from `~/code/qrtz`. Raw WER identical; formatted WER equal
   or lower on every clip. `git -C ~/code/qrtz worktree remove /tmp/qrtz-c66`.
4. Hold fn and say "So we, we can't make it to the dinner" (stumble on "we" on purpose): it types "So we can't make it
   to the dinner.". Then "So we, we candidly told them": both words stay. Then "Neither have we, we can't wait": both
   words stay.
5. If anything reads worse: revert `dd2a416` (talk-only commit; its vectors and tests go with it).
6. Still open for the next cycle's p15 decision: hold fn and say "Bring the charger comma the cable comma and the
   adapter period"; note whether Parakeet writes the words or the marks (section 36 step 5).

## 40. Cycle 68: "Hey <dictionary name> did you <listed verb>" is a question (ADR 0208, `6f1daed`)

| Run                | Exact     | Edit distance     | Regressions | False-positive hits |
| ------------------ | --------- | ----------------- | ----------- | ------------------- |
| 7c008f4, 105 cases | 101 / 105 | 42 chars (1.27 %) | -           | 0 / 275             |
| 6f1daed, same 105  | 101 / 105 | 42 chars (1.27 %) | 0           | -                   |
| 7c008f4, 109 cases | 101 / 109 | 50 chars (1.43 %) | -           | -                   |
| 6f1daed, 109 cases | 105 / 109 | 42 chars (1.20 %) | 0           | 0 / 292             |

Regressions are counted against `7c008f4`'s own output for every case (p106-p109's old output was committed with the
code; `polish-baseline.json` then refreshed to `6f1daed` in the docs commit); 4 cases better (p106-p109), none worse.

- ADR 0208: "Hey Jared did you finish the report" → "Hey Jared, did you finish the report?"; also "hey priya did you
  hear back from the landlord", "... Hey Jared did you read it yet", "Hey pria did you book the flights for Friday".
  `VOCATIVE_DID_VERBS` (53 base-form verbs that never stand bare as the complement of "did you") joins the 25 plain
  verbs only after "Hey <name> did you"; every other ADR 0206 guard unchanged; a term spelled like a listed verb is
  never a name. "Hey Jared did you a solid / good / wrong / proud / justice / dirty / like a pro / a good turn / right
  / no harm / well / credit ..." keep "."; so do "Hey Sam ...", "Okay Jared ...", an existing comma, "ever", "So I said
  hey Jared ...", and a later clause. 4 polish positives, 17 new keep vectors, cleanup.json 1 flipped + 28 new (Rust
  and TS same text).
- Cost (release, box CPU; old and new `cleanup_budget` binaries built one after the other, copied to /tmp,
  interleaved 15×, median of medians): 495 words 0.131 → 0.134 ms; 504 words ending in a question 0.143 → 0.142 ms;
  504 words ending in a vocative question 0.149 → 0.153 ms. Noise; ceiling 2 ms.
- Path review: origin/main had not moved since `7c008f4`; no capture, VAD, model config, drain, insert, pill or
  timing-log change (talk's cleanup is still the only post-processing, cleanup.rs `rule_cleanup` ~l.1927, run once
  on the final text; the vocative step is `vocative_question` ~l.1707, called from `terminal_punctuation`). Findings
  list unchanged; backlog P1-P8 still open. Note (Qrtz Nano): packages/features/note vitest green (with talk, 15
  files, 283 tests).

Still not exact (4 of 109): p15 spoken "comma"/"period", p19/p20 email and URL, p41 a numbered list with a 4-word line.

Rejected this cycle (details in ADR 0208): "did you" before any word; like/turn/forward/save/close/clean/open/fine/
miss/love/harm/damage/service/honor/credit on the list; the same list after a bare "So"; still-rejected ADR 0206
shapes; p15 (still waiting on the Mac check).

Next polish candidates, ranked: (1) p15 spoken "comma"/"period", only after the Mac check in section 36 step 5. (2)
None other above the bar on the current set; p19/p20 (spoken email/URL) and p41 (a numbered list with a 4-word line)
need their own evidence. Next cycle: backlog P1 (the qrtz-app `local_models::speech_calls_scoped_to_one_module_touch_
only_its_models` timeout, flake vs hang, root cause).

### Measurement plan for Jared's Mac (cycle 68; proves no added latency and no worse transcript)

Talk-only change (`6f1daed`); the helper does not run talk's cleanup, so `echo-bench.sh` timings can't move from it.

1. `cd ~/code/qrtz && git pull` (main at or after `6f1daed`).
2. `cargo test --release -p qrtz-talk --test cleanup_budget --test hardening60 --test hardening68 --test polish_set --
   --nocapture`: three budget medians well under 0.5 ms (box 0.13-0.15 ms), polish 105/109 exact, regressions 0, all
   green.
3. Score the same raw reports with the old cleanup: `git -C ~/code/qrtz worktree add --detach /tmp/qrtz-c67 7c008f4 &&
   cd /tmp/qrtz-c67 && cargo run -q -p qrtz-talk --example echo_score -- ~/qrtz-echo-bench/results/<run>-<stamp>.json`,
   keep a copy of `<report>.scored.json`, then score again from `~/code/qrtz`. Raw WER identical; formatted WER equal
   or lower on every clip. `git -C ~/code/qrtz worktree remove /tmp/qrtz-c67`.
4. With a name in Echo's dictionary (for example "Priya"), hold fn and say "Hey Priya did you finish the report": it
   types "Hey Priya, did you finish the report?". Then "Hey Priya did you a favor last week": it types "Hey Priya did
   you a favor last week." (no comma, "."). A name not in the dictionary ("Hey Sam did you finish the report") types
   as before.
5. If anything reads worse: revert `6f1daed` (talk-only commit; its vectors and tests go with it).
6. Still open for p15: hold fn and say "Bring the charger comma the cable comma and the adapter period"; note whether
   Parakeet writes the words or the marks (section 36 step 5).

## 41. Cycle 69: backlog P1, a speech download request while one runs is never lost (ADR 0209, `a8da381`)

No polish item cleared the bar (section 40: p15 waits on the Mac check; p19/p20 and p41 need their own evidence), so
the cycle took backlog P1.

| Run                                                    | Result                                             |
| ------------------------------------------------------ | -------------------------------------------------- |
| `9ee8fb4`, P1 test, 20 sequential runs                 | 14 pass (~1.3 s), **6 hang 30 s** "nano installed" |
| `a8da381`, P1 test, 60 sequential runs                 | 60 pass, max 1.38 s                                |
| `a8da381`, the 5 download tests, 4 parallel loops × 15 | 60 / 60 pass                                       |
| `9ee8fb4`, the 2 new deterministic tests, 3 runs each  | fail every run                                     |

- **Verdict: a real hang, not a flake.** `SpeechManager::download_for` (setup.rs l.294-300 at `9ee8fb4`) only
  cleared the pause flag while a download was `running`; an attempt that had already seen the pause then ended
  paused (or as a "network" error) and nothing ran again. A request for other models while one ran was dropped
  outright (the test's second hang point and its 100 ms sleep before the delete). Ruled out: slow box (settled state
  in every hung run), network / index refresh (loopback only, no refresh for these sets), deadlock, blocking in
  async, filesystem waits (ADR 0209).
- **Fix:** a run gate under which a request and the running task's end are decided; a request while running is kept
  and run when the attempt ends (a pause after it stands; a plain repeat of what just finished is done); the row and
  marker stop saying paused at once; pause/delete decide under the gate.
- **Tests:** `a_resume_while_the_paused_download_winds_down_is_not_lost`, `a_download_asked_while_another_runs_runs_
  after_it` (fail every run on `9ee8fb4`), `a_pause_after_a_waiting_resume_stands`; qrtz-app (`--features runtime`)
  290 passed, 0 failed; clippy `-D warnings` (all targets, with and without `runtime`); rustfmt clean on the touched
  files.
- **Dictation path review since `7c008f4`:** only talk cleanup moved (`6f1daed`); capture, VAD, model config,
  drain, insert, pill and the timing log unchanged; this cycle's change is not on the press path. Polish set
  re-run at `a8da381`: 105 / 109 exact, 42 chars, regressions 0; keep set 292, 0 hits; release cleanup_budget 0.137
  / 0.144 / 0.153 ms median (495 words / 504 ending in a question / 504 ending in a vocative question), ceiling 2 ms.
  Note (Qrtz Nano): packages/features/note vitest 17 passed (5 files).
- **Latency:** none. No change to chunking, segmenting, the decode schedule, capture or insertion timing.

Backlog after this cycle, ranked: (1) **P3** an installed and verified Parakeet never touches or waits on the
network: `LocalModels::download` and `speech:status` must not refresh the index when Parakeet is installed and
verified, with a test with the network down; (2) P2 crash recovery mid-download (resume from the `.part` + `.done`
ranges, sha256 on the result) with a kill test; (3) P5 the key-up-to-text p50/p95 bench-compare script; (4)
P4/P6-P8. Polish: p15 only after the Mac check in section 36 step 5.

### Test on Jared's Mac (cycle 69)

1. `cd ~/code/qrtz && git pull` (main at or after `a8da381`).
2. `cargo test -p qrtz-app --features runtime --test local_models`: 7 passed, 1 ignored (the real-host download).
3. `for i in $(seq 1 30); do cargo test -q -p qrtz-app --features runtime --test local_models speech_calls_scoped ||
   break; done`: every run passes, each in ~1-2 s (before the fix about 1 in 3 hung 30 s).
4. Optional, in the app (only if some module's models aren't installed yet, e.g. nano switched off and its models
   deleted): start the download, press pause and then resume within a second, several times. The row says
   downloading right after each resume and the download finishes; nothing is left paused or "couldn't reach the
   download server".
5. If anything misbehaves: revert `a8da381` (setup.rs and the tests only).

## 42. Cycle 70: backlog P3, an installed and verified Parakeet never touches or waits on the network (ADR 0210, `de03608`)

Polish re-check first: none of the 4 failing cases has an unambiguous rewrite. p15 (spoken "comma"/"period") still
waits on the Mac check (section 36 step 5); p19 "jared at gmail dot com" → email fails on "look at gmail dot com"
(the same words, "at" a preposition) and ADR 0190 keeps Parakeet's ITN off; p20 "qrtz dot ai slash docs" → URL
fails on "the dot com bubble" and has no measured Parakeet output; p41 (a 4-word list line) is ADR 0188's "don't
retry". So the cycle took backlog P3.

| Run                                                      | Result                                                     |
| -------------------------------------------------------- | ---------------------------------------------------------- |
| `4fe0780`, the 2 new tests                               | **both fail** ("another install is running" / "signature") |
| `de03608`, the 2 new tests                               | pass, 0 connections of their own, each under 2 s           |
| `de03608`, local_models (10 tests), 175 runs (see below) | 175 pass                                                   |
| `de03608`, qrtz-app `--features runtime`                 | 292 passed, 0 failed (was 290)                             |

The 175 runs: 25 + 30 via cargo, 60 of the test binary, 4 parallel loops × 15. A first loop of 20 via cargo had 1
failing run whose output wasn't kept; it did not recur in the 175.

- **What was already offline (unchanged):** the press path asks only the disk (talk.rs `echo_speech_missing`
  ~l.113: marker + sizes); `speech:status` for the local models is disk only (setup.rs `status_for` ~l.217);
  Parakeet's download never refreshes the index (only an extension-read set like ctrl's Laya does,
  local_models.rs ~l.387). With the network merely down, an installed Parakeet's download made no connection.
- **N188 (P3, fixed):** with Parakeet installed and verified, `LocalModels::download` still ran an empty
  `Installer::apply`, which takes `.qrtz-install.lock` (qrtz-modules install.rs ~l.861). `Installer::refresh` holds
  that lock across its network requests (install.rs ~l.466; 15 s connect / 60 s read timeout per request). So during
  any refresh stuck on a dead network (the qrtz page's refresh, "check for updates", `qrtz update`, ctrl's Laya
  download), echo's `speech:download` failed "another install is running", shown as "the download failed its
  signature check", for a model that was installed.
- **Fix:** `download` asks `up_to_date` first (local_models.rs ~l.378 / ~l.446): every set marked with the right
  sizes, no extension-read set, and the pinned plan a no-op that leaves `qrtz.lock` as it is (disk only, no lock).
  Then it reports full progress and returns: no transport, no refresh, no lock. Anything else takes the old path.
- **Tests:** `an_installed_parakeet_downloads_with_the_network_down_without_touching_it`, `speech_status_and_
  download_with_parakeet_installed_never_wait_on_the_network` (a loopback black hole that accepts and never answers
  is the model and package host; a real `Installer::refresh` is stuck on it holding the lock). clippy `-D warnings`
  all targets with and without `runtime`; rustfmt clean on the touched files.
- **Dictation path review since `7c008f4` / `a8da381`:** no capture, VAD, model config, drain / final pass, cleanup,
  insert, pill or timing-log change. origin moved by `82d75ae` (echo bench plan: echo vs another dictation app only, scripts and
  docs), `f560680` (cargo fmt of modules.rs / packages_ipc.rs) and `df5bb62` (app-update key pin, ADR 0181); none on
  the press path. Cleanup is still `rule_cleanup` (cleanup.rs ~l.1927) once on the final text,
  `terminal_punctuation` ~l.1568, `vocative_question` ~l.1707. Polish set at `de03608`: 105 / 109 exact, 42 chars
  (1.20 %), regressions 0; keep set 292, 0 hits; release cleanup_budget medians over 5 runs 0.121 / 0.125 / 0.131 ms
  (495 words / 504 ending in a question / 504 ending in a vocative question), ceiling 2 ms. Note (Qrtz Nano):
  packages/features/note vitest 17 passed (5 files).
- **Latency:** none. Nothing on the press path changed; the download of an installed model got cheaper.
- **N189 (P3, new; fixed in cycle 71, `b464db9` + `7cf1272`, section 43):** a download that does need bytes while a refresh is stuck still fails "busy" and is shown
  as a signature failure (`pm_error`, local_models.rs ~l.617, maps `InstallError::Busy` to `Invalid`); and a stuck
  refresh blocks every install for up to ~60 s per request. Proposed to Beve Bobs: refresh without holding the
  install lock during its requests (ADR 0210, Rejected).

Backlog after this cycle, ranked: (1) N189's speech side (mine): `InstallError::Busy` from a speech download shows as
"another download or update is running; try again in a moment" (`errorKind` `unavailable`, already in packages/core
`SpeechErrorKind`) and retries itself once the lock frees, with a test; (2) P2 crash recovery mid-download (resume
from the `.part` + `.done` ranges, sha256 on the result) with a kill test; (3) P5 the key-up-to-text p50/p95
bench-compare script; (4) P4/P6-P8. Polish: p15 only after the Mac check in section 36 step 5.

### Test on Jared's Mac (cycle 70)

1. `cd ~/code/qrtz && git pull` (main at or after `de03608`).
2. `cargo test -p qrtz-app --features runtime --test local_models`: 9 passed, 1 ignored (the real-host download).
   The two new tests each finish in about 1 s.
3. `for i in $(seq 1 20); do cargo test -q -p qrtz-app --features runtime --test local_models parakeet || break;
   done`: every run passes.
4. In the app, with Parakeet installed: turn Wi-Fi off, quit qrtz and open it again. Echo's page and settings →
   speech say installed, with no "couldn't check speech" or "signature" error; hold fn and dictate "Testing offline
   dictation one two three": it types as usual, with no wait.
5. Still with Wi-Fi off: on the qrtz page press refresh / "check for updates" and, while it is still trying, open
   echo's page and switch echo off and on. Echo comes back on at once with no download question and no error row.
6. Turn Wi-Fi back on. If anything misbehaves: revert `de03608` (local_models.rs and its tests only).
7. Still open for p15: hold fn and say "Bring the charger comma the cable comma and the adapter period"; note
   whether Parakeet writes the words or the marks (section 36 step 5).

## 43. Cycle 71: backlog P3 / N189 speech side, a model download waits for the install lock (ADR 0213, `7cf1272`)

Polish re-check first: nothing new clears the bar. The 4 failing cases are the same as in section 42 (p15 waits on
the Mac check in section 36 step 5; p19/p20 are ambiguous; p41 is ADR 0188's "don't retry"), and no cleanup code
changed since `de03608`. So the cycle took the top backlog item from section 42: N189's speech side. Bobs's
`b464db9` already fixed the store side (a refresh takes the install lock only to commit).

| Run                                                       | Result                                                                 |
| --------------------------------------------------------- | ---------------------------------------------------------------------- |
| old mapping + no wait, the 3 new tests                    | **all fail** (`Invalid("another install is running")`)                 |
| `7cf1272`, the 3 new tests                                | pass (waits, then installs; `InstallBusy` after the wait; pause < 1 s) |
| `7cf1272`, local_models (13 tests), 40 runs of the binary | 40 pass                                                                |
| `7cf1272`, qrtz-app `--features runtime`                  | 303 passed, 0 failed                                                   |

- **N189 (speech side, fixed):** a Parakeet (or nano model) download that needed bytes while another install held
  `.qrtz-install.lock` failed at once. `pm_error` turned `InstallError::Busy` into `SpeechError::Invalid`, so the row
  said "the download failed its signature check".
- **Fix:** `LocalModels::download` (local_models.rs ~l.440) plans and applies in a loop. On `Busy` it waits 200 ms
  and plans again, for up to 60 s (`BUSY_WAIT`), and a pause ends the wait at once. Past that, the error is
  `SpeechError::InstallBusy` (retryable), kind `unavailable`, with the row text "another download or update is
  running; try again in a moment" (setup.rs `error_kind` / `error_text`). No renderer or contract change, since
  `unavailable` is already a `SpeechErrorKind`.
- **Dictation path review since `de03608`:** no capture, VAD, model config, drain / final pass, cleanup, insert or
  timing-log change in qrtz-talk. origin moved by the pill animation (`e330e9f`) and theme (`d21947c`; helper, drawn
  after capture starts, nothing awaited), the onboarding rows (`a0a09aa`), ADR 0212's declared model reads
  (`2b423fc`..`8a84a06`, including local_models `reader_of` from `qrtz.lock` and Models.swift's `QRTZ_MODELS_DIR`
  unwrap), Bobs's refresh lock (`b464db9`) and publish-local scripts. Cleanup is still `rule_cleanup` once on the final
  text. Polish set at `7cf1272`: 105 / 109 exact, 42 chars (1.20 %), regressions 0; keep set 292, 0 hits; release
  cleanup_budget medians 0.128 / 0.130 / 0.137 ms (495 words / 504 ending in a question / 504 ending in a vocative
  question), ceiling 2 ms. Note (Qrtz Nano): packages/features/note vitest 17 passed (5 files).
- **Latency:** none. Only the download path changed, and an installed model returns before the loop.

Backlog after this cycle, ranked: (1) P2 crash recovery mid-download (resume from the `.part` + `.done` ranges,
sha256 on the result) with a kill test; (2) P5 the key-up-to-text p50/p95 bench-compare script; (3) P4/P6-P8.
Polish: p15 only after the Mac check in section 36 step 5.

### Test on Jared's Mac (cycle 71)

1. `cd ~/code/qrtz && git pull` (main at or after `7cf1272`).
2. `cargo test -p qrtz-app --features runtime --test local_models`: 12 passed, 1 ignored. The three new tests
   (`waits_for_another_install`, `lock_held_past_the_wait`, `pausing_while_waiting`) finish in about 1 s together.
3. Optional, in the app (only if nano is on but its models aren't downloaded yet): start nano's model download in
   settings → speech. While it runs, switch echo off and on (or delete and re-download Parakeet). Echo's row stays on
   "downloading" and finishes after nano's, with no "signature check" error.
4. If anything misbehaves, revert `7cf1272` (local_models.rs, setup.rs, qrtz-speech-types and the tests only).
5. Still open for p15: hold fn and say "Bring the charger comma the cable comma and the adapter period", and note
   whether Parakeet writes the words or the marks (section 36 step 5).

## 44. Cycle 72: backlog P2, a download cut short by a crash or a kill resumes cleanly (ADR 0218, `b88bae3`)

Polish re-check first: nothing new clears the bar. No cleanup code changed since `342e67b` (cleanup.rs, cleanup.ts
and spec/vectors/talk are byte-identical), and the 4 failing cases are the same as in section 43 (p15 waits on the
Mac check in section 36 step 5; p19/p20 are ambiguous; p41 is ADR 0188's "don't retry"). So the cycle took the top
backlog item: P2, crash recovery mid-download.

| Run                                                       | Result                                                                                                                           |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| old fetch.rs, the 5 new tests                             | **4 fail** (marks read `10`; `bytes didn't match`; permission denied; the bound guard by its retry count); the kill guard passes |
| old fetch.rs, torn-mark test with its marks check removed | **fails** at the next resume: `every source failed (bytes didn't match)`                                                         |
| `b88bae3`, fetch_resume (5 tests), 20 runs                | 20 / 20 pass (~5 s each; the kill test is a real SIGKILL)                                                                        |
| `b88bae3`, qrtz-modules / qrtz-talk / qrtz-app `runtime`  | 98 / 148 / 305 passed, 0 failed (2 ignored in qrtz-app)                                                                          |

- **What already held (unchanged):** a clean quit or a pause resumed from `store/.tmp/<sha256>.part` + `.done`
  (fetch.rs `fetch_one`); setup.rs writes `download.json` before the download and `resume_pending` restarts it at
  launch; `.qrtz-install.lock` is a `flock` (install.rs `lock` ~l.373, `try_lock`), so a crash never leaves it held;
  a part with the wrong size starts over; nothing enters the store unless the whole file matches the signed sha256.
  The new kill test proves the main path end to end across processes.
- **N190 (fixed):** a torn range mark merged with the next one. `writeln!(f, "{i}")` is two writes (checked:
  `["12", "\n"]`). A kill between them left `1` with no newline, the next fetch appended `0\n`, and the marks read
  range 10, which was zeros. The resume after that failed its hash. Fix: one `write_all` per mark (`append_mark`
  ~l.96), only newline-terminated numbers count (`read_marks` ~l.75), and a torn file is rewritten atomically as
  whole lines before anything is appended (~l.250).
- **N191 (fixed):** a kept part that failed its sha256 failed the whole download, because a model file has one
  source (its pinned Hugging Face URL, ADR 0171), even when the bad bytes were left by an earlier process. Fix: a
  blob that had a full-size part with marks before this fetch (`kept_ranges` ~l.204) gets one fresh try from the
  same source (~l.175). A fresh download with wrong bytes is not retried. A source that keeps serving wrong bytes
  still fails after the one try.
- **N192 (fixed):** a crash between `insert_file`'s chmod 0444 and its rename, with marks lost, left a part that
  every later fetch kept and couldn't write ("permission denied" forever). Fix: a kept part is set to 0600 (~l.247).
- **N193 (open, Bobs's area):** `Installer::sweep` never looks in `store/.tmp/`. A part for a blob nothing wants
  any more stays (an interrupted download followed by delete, or a new model pin). It's sparse, so it holds only the
  ranges that landed. Proposal below.
- **Dictation path review since `342e67b`:** cleanup.rs unchanged (`rule_cleanup` l.1927, `terminal_punctuation`
  l.1568, `vocative_question` l.1707), so is the TS twin. No capture, VAD, model config, drain / final pass, insert,
  pill or timing-log change for a plain fn press. origin moved by ctrl do (`6006bf0`..`663fdac`, helper ctrl only),
  `f1bc172` (feed.mjs), `f8d370b` (Package.resolved), nano calendar (`566c954`..`190b7ff`), and `85a126f` ctrl+fn:
  talk service.rs records `press_ctrl` at `down` (l.765) and an attempt is `to_ctrl` only when started from the key
  with ctrl held (l.1213). The cleaned text then goes to ctrl do instead of the insert (l.1553). A plain fn press
  reads one bool and takes the old path; the cleanup still runs once on the final text. No new finding there.
- **Scores at `b88bae3`:** polish set 105 / 109 exact, 42 chars (1.20 % of 3504), regressions 0; keep set 292, 0
  hits; release cleanup_budget medians over 5 runs 0.109 / 0.122 / 0.130 ms (495 words / 504 ending in a question /
  504 ending in a vocative question), ceiling 2 ms. TS twin (packages/core/src/talk vitest) 266 passed (10 files).
  Note (Qrtz Nano): packages/features/note vitest 17 passed (5 files).
- **Latency:** none. Only the fetcher changed. A mark is now one write instead of two, the marks file is rewritten
  only after a torn line, and the press path never downloads.

Proposal for Beve Bobs (N193, qrtz-modules install.rs):

> `Installer::sweep` never looks in `store/.tmp/`, so a `<sha256>.part` + `.done` for a blob nothing wants any more
> stays forever. Example: echo's Parakeet download is cut short (quit, crash), then the user deletes the model, or
> the pin moves to a new revision. The part is sparse, so it holds only the ranges that landed, up to 445 MB for
> Parakeet's largest file. Proposed: in `sweep` (it runs under the install lock, so no fetch is writing), with
> `delete`, remove `store/.tmp/<sha256>.part` / `.done` whose sha256 no locked tree names and no `LocalModels`
> manifest set names (that keeps a part a pending `download.json` will resume), plus `*.in-<pid>` and `*.sideload`
> leftovers older than an hour; count their on-disk bytes in `reclaimable()` so the "clean up" button and the
> prune agree. Expected gain: the disk an abandoned download used. Measure on Jared's Mac: start echo's Parakeet
> download, `pkill -9 -x qrtz-app` at about 50 %, reopen, pause, delete echo's model, press "clean up", then
> `du -sh ~/Library/Application\ Support/ai.qrtz.desktop/store/.tmp` (before: about half the model; after: ~0).
> Latency: none, since sweep never runs on the press path. Transcripts: unchanged, since no store blob or model
> view is touched and the helper never reads `.tmp`.

Backlog after this cycle, ranked: (1) P5 the key-up-to-text p50/p95 bench-compare script for Jared's Mac bench
output; (2) N193 (Bobs: sweep stale `store/.tmp` parts under the install lock, keeping one a pending `download.json`
will resume); (3) P4/P6-P8. Polish: p15 only after the Mac check in section 36 step 5.

### Test on Jared's Mac (cycle 72)

1. `cd ~/code/qrtz && git pull` (main at or after `b88bae3`).
2. `cargo test -p qrtz-modules --test fetch_resume`: 5 passed in about 5-10 s. The kill test starts a copy of the
   test binary and kills it with SIGKILL mid-download.
3. `for i in $(seq 1 10); do cargo test -q -p qrtz-modules --test fetch_resume || break; done`: every run passes.
4. `cargo test -p qrtz-app --features runtime --test local_models --test setup`: all pass (12 + 1 ignored, 6).
5. Optional, in the app (it re-downloads Parakeet, 464 MB): opts (formerly settings) → speech → delete echo's model, then download
   it. At about 30-50 %, run `pkill -9 -x qrtz-app` (a crash, not a quit). Open qrtz again: the download resumes by
   itself near where it stopped, not from 0, finishes, and echo dictates "Testing crash recovery one two three".
   Then `ls ~/Library/Application\ Support/ai.qrtz.desktop/store/.tmp/` shows no `.part` or `.done` left for it.
6. If anything misbehaves, revert `b88bae3` (fetch.rs and its test only).
7. Still open for p15: hold fn and say "Bring the charger comma the cable comma and the adapter period", and note
   whether Parakeet writes the words or the marks (section 36 step 5).

## 45. Cycle 73: backlog P4, a mic error for another press never ends a finishing dictation (ADR 0222, `88c97ae`)

Polish re-check first: nothing new clears the bar. cleanup.rs, cleanup.ts and spec/vectors/talk are unchanged since
`342e67b`, and the 4 failing cases are the same as in section 44 (p15 waits on the Mac check in section 36 step 5;
p19/p20 are ambiguous; p41 is ADR 0188's "don't retry"). So the cycle took the next backlog item: P4, mic edge
cases, talk side.

| Run                                                     | Result                                                                                                                               |
| ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| old service.rs (origin/main `9a252d9`), hardening73 (3) | **3 fail**: finalizing → a new dictation listening, words lost; inserting → no history entry; another id's error fails the dictation |
| `88c97ae`, hardening73                                  | 3 / 3 pass                                                                                                                           |
| `88c97ae`, qrtz-talk (38 test binaries)                 | 153 passed, 0 failed; clippy `-D warnings` all targets and rustfmt clean                                                             |

- **What P4 already had, talk side (unchanged):** the helper ends a dictation itself on a revoked mic or a device
  that doesn't come back (`dictation.stopped{reason: permission-revoked | mic-lost}`); talk keeps and inserts what
  was heard and then says why (service.rs `dictation.stopped` l.838, `ended_by_helper`). A device switch (AirPods
  connect or drop, a USB mic, Control Center) re-opens the input on a fresh engine within ~1 s (MacAudio.swift
  `MicCapture.changed`, N138) and the dictation goes on. A lost helper ends the dictation with what was heard
  (`helper_lost` l.972). A revoke or loss during finalizing is answered by the stop with its reason
  (Dictation.swift `abort`, `s.closed`).
- **N194 (fixed `88c97ae`):** `error{dictation.mic}` names no dictation, and the helper also sends it when a new
  press's arm fails to open the mic (`EchoController.pushKey(.down)` l.148 → `configure(warmMic: true)` →
  `attachMic` → `startMic` l.321), which `canArm` allows while the last dictation is finalizing. Talk failed any
  active dictation (l.887 before): a finalizing one was cancelled in the helper (its final dropped), its stop wait
  released with nothing and its attempt cleared. The words were lost with no history entry, the pill said
  "microphone access denied", and the queued press then started a fresh dictation. Repro: release, pull the USB
  mic, press again within the finalize. Fix: only a `listening` dictation (the one the error names, if it names
  one) fails; the press reports the mic through its own start.
- **N195 (open, Bobs; likely, verify on the Mac):** with echo's managed mic (`managedMic: true`,
  EchoController.swift l.74), `Dictation.start` doesn't open the mic itself (l.399); the arbiter's
  `mic.subscribe:dictation` does. If that `startMic` threw at the arm, `micRunning` stays false while the arbiter
  may count the dictation reader as subscribed. The next start then gets no new subscribe effect, runs with no mic,
  and ends in "didn't catch any words" instead of a mic error. Proposal below.
- **N196 (open, Bobs):** no sleep/wake handling in the helper (no `willSleepNotification` / `didWakeNotification`
  observer in QrtzEcho / QrtzEchoMac / QrtzMacShared). A push key held when the lid closes can lose its key-up: the
  pill stays listening after wake until the next press and release. That release does end it, so it isn't stuck
  for good, but it looks stuck. Proposal below.
- **N197 (open, talk side, not landed):** a mic that stops delivering blocks without a configuration change or a
  loss (no `dictation.level` for seconds) keeps the pill listening; the release still ends it with what was heard.
  A talk-side "no level for N s" watchdog would be mine, but N needs the Mac's real device-switch gap first
  (`MicRestart` retries for ~1 s), or it could end a dictation that would have recovered. Below the bar this cycle.
- **Notch surface review (`7f518b6`, ADR 0221) against the pill state:** no stuck-pill path. The pill's phases
  still come from `PillDriver` (QrtzEcho/Pill.swift, its state machine and error-flash timeout unchanged; `notch.*`
  only passes through at l.204). `NativePill` maps `hidden` to `dismiss(owner: echo.dictation)`, and the surface
  drops every host `notch.show/update/dismiss` naming that owner or `system` priority (NotchSurface.swift `host`
  l.87). A dismiss mid-grow retracts; a show mid-retract grows back (`hideGeneration`). Talk's only new code is
  `notch_send` (a type filter); nothing calls it or `NotchBroker` yet. **N198 (low, Bobs):** `NotchBroker::tick`
  (notch.rs l.392) is what expires module content after 10 s and isn't wired, so wire it before the module IPC
  that feeds the broker, or a module entry stays under dictation for good. `NotchArbiter.dismiss(owner:)` takes
  the first entry with that owner at any priority (Notch.swift l.137); that's fine while the broker gives an owner
  one priority.
- **Dictation path review since `67b3d58`:** no capture, VAD, model config, drain / final pass, cleanup or
  timing-log change for a plain fn press. `389e383` (ctrl+fn H-C2) touches `stop` / `finish_inner` only behind
  `to_ctrl`. Nano notes, ctrl, the 0.2.3 release, feed.mjs and the modules publish are off the press path.
- **Scores at `88c97ae`:** polish set 105 / 109 exact, 42 chars (1.20 % of 3504), regressions 0; keep set 292, 0
  hits; release cleanup_budget medians over 3 runs 0.115-0.129 / 0.118-0.148 / 0.126-0.151 ms (495 words / 504
  ending in a question / 504 ending in a vocative question), ceiling 2 ms, the benchmark test passes. TS twin
  (packages/core/src/talk) plus Note (packages/features/note) vitest: 290 passed (16 files).
- **Latency:** none. One phase check on an error line; nothing on the press, release, cleanup or insert path.

Proposal for Beve Bobs (N195, N196; helper, QrtzEcho / QrtzEchoMac):

> Two mic edge cases from backlog P4. Talk now ignores an id-less `error{dictation.mic}` unless a dictation is
> listening (ADR 0222, `88c97ae`), so these are what's left on the helper side.
>
> **N195, a failed arm mic open.** With `managedMic: true`, `Dictation.start` doesn't call `startMic` (Dictation.swift
> l.399); the arbiter's `mic.subscribe:dictation` does, from `pushKey(.down)` → `configure(warmMic: true)`
> (EchoController.swift l.148). Please check first: when `startMic` throws there (l.321), does the arbiter still
> count dictation as a mic reader? If so the next `dictation.start` gets no subscribe effect and runs with no mic
> ("didn't catch any words" after the release). Proposed: on a throw in `startMic` from `attachMic`, step the
> arbiter back (`mic.unsubscribe:dictation`, or a `configure(warmMic: false)`) so the next start subscribes again.
> And in `Dictation.start` with a managed mic, if `!micRunning` after the admit, call `startMic()` once; if it
> throws, send `error{dictation.mic, id}` with the start's id (talk keys on it) instead of running deaf. Nice to
> have: a `reason` field (`denied` | `no-device`) so talk can type it `mic-lost` instead of `mic-denied`.
>
> **N196, sleep/wake with the key held.** No observer for `NSWorkspace.willSleepNotification` /
> `didWakeNotification` today. Proposed: on will-sleep, if the push key is held, send `hotkey{action: up}` (the
> same line a real release sends), so the dictation stops and inserts what was heard before the lid closes. On
> did-wake, if the helper still thinks the key is held, read `CGEventSource.flagsState(.combinedSessionState)` and
> send `up` when the fn / Globe flag is clear. Talk already handles a lone `up` (it only ends what a press started,
> N172).
>
> Expected gain: no "didn't catch any words" after a mic pull and re-press; no pill left listening after a lid
> close. Latency: none on the press path. The arm's open runs as today; the only additions are a check after a
> failed open and two notification observers.
>
> Mac measurement plan: (1) Release fn, pull the USB mic (or turn AirPods off), press again within ~0.5 s and
> hold 2 s. Before (if N195 holds): the second dictation ends "didn't catch any words". After: a mic error at once. The first
> dictation's text goes in both times (that's `88c97ae`). (2) Hold fn, close the lid, wait 10 s, open it. Before:
> the pill is still listening. After: the text heard before the close is inserted (or on the clipboard), and the
> pill is gone. (3) Zero latency change: 20 normal dictations before and after, then
> `node scripts/echo-timing-report.mjs --since 2026-10-19` on ~/Library/Logs/qrtz. The key-up → text p50 / p95
> from the `echo timing:` / `talk latency` lines must be within noise. (4) No worse transcripts:
> `bash native/qrtz-helper/scripts/echo-bench.sh --quick` before and after; WER must be identical (the recogniser
> path is untouched).

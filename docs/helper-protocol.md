# Helper child protocol: concurrency and permission revocation

The wire format is newline-delimited JSON over stdio. The source of truth is
`packages/core/src/helper/protocol.ts`, mirrored in Swift by
`native/qrtz-helper/Sources/QrtzHelperCore/Protocol.swift`. Golden lines live in
`native/qrtz-helper/Fixtures/protocol/*.jsonl`, and the generated JSON schema is in `contracts/helper-protocol.json`.
This document covers two areas the per-message schema can't express:

- what happens when `recording.*` and `dictation.*` overlap on one helper (H6)
- what happens when a TCC permission is withdrawn mid-capture (H5)

Both rules live in a pure arbiter:

- TS: `packages/core/src/helper/arbiter.ts`, used by the mock helper and the host
- Swift: `QrtzHelperCore/Arbiter.swift` (`SessionArbiter`, `MicHub`), used by the real helper

The two implementations are checked against the same vectors in `spec/vectors/helper/arbiter.json`, by
`arbiter.test.ts` and by `ArbiterTests.testSharedVectors`. Change the vectors first, then both implementations.

## Concurrency (H6): mux

Options considered:

| Option  | Behaviour                                               | Why not / why                                                                                                                 |
| ------- | ------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| refuse  | `dictation.start` during a recording → error            | Dictating into Slack or Notes while a meeting records is the main use case. Refusing breaks it.                               |
| queue   | Hold the second start until the first finishes          | Push-to-talk needs a reply within ~100 ms. A queued dictation is effectively a refused one, and queued meeting audio is gone. |
| **mux** | Both run, and the mic is read once through a shared tap | **Chosen.** One input device, two readers. The meeting adds system audio, and dictation stays mic-only.                       |

Rules (the arbiter's `cmd` events):

1. `recording.*` and `dictation.*` are independent sessions. Starting or stopping one never starts, stops or restarts the other.
2. Each kind is single-instance. A second `recording.start` while recording, or `dictation.start` while dictating, is
   **ignored** (it is not queued) and answered with a busy error that names both the ignored start and the session
   that owns the helper:
   `error{code: "dictation.busy", id, activeId, fatal: false}` and
   `error{code: "recording.busy", audioPath, activeAudioPath, fatal: false}`, so a host waiting for `started` fails at
   once instead of timing out (and a later `stop` for that id can't hang). The host acts on a busy only when `id` /
   `audioPath` is its start in flight; a busy for any other start is logged and ignored, and it never cancels by its
   current id when the busy names a different one. A busy whose active id / path equals the requested one is a
   duplicate start of a running session: the host keeps it. If the active session is unknown to the host (it outlived
   a host reload, or an abandoned attempt), the host sends `dictation.cancel{id: activeId}` or
   `recording.stop{audioPath: activeAudioPath}` before failing the attempt (`helper-busy`, retryable), so the orphan
   is cleaned up and the retry succeeds. The host never stops a recording it is capturing itself. A refused
   dictation is not cancelled by its own id (the helper never ran it). Stops and cancels when nothing is running are
   no-ops (`not-recording` / `not-dictating`). `dictation.stop` and `cancel` with a stale id are no-ops too.
   `recording.stop` takes an optional `audioPath`: when it names a file other than the one being recorded, the stop is
   ignored (logged). The host sends the path on every cleanup stop (a stale `recording.started{A}`, a start timeout),
   so a late ack for an abandoned start never stops the recording that replaced it.
3. Shared mic tap (`MicHub`). The physical mic (one `AVAudioEngine` input tap) starts on the first reader and stops on
   the last. Readers are `recording` (the capture source's mic leg, mixed with system audio and echo-gated) and `dictation`.
   Every reader gets every block. When both are running, the helper logs `mux: recording and dictation share the mic tap`.
   The warm mic keeps a `dictation` reader subscribed between dictations (see Warm mic below).
   The device start is race-safe: `source.start` runs outside the hub lock, a subscribe during it joins and waits for
   the outcome, and if every reader left (or the tap was dropped) by the time it returns, the hub stops the device
   again. If `source.start` throws, every reader of that start (the starter and anyone who joined during it) is
   detached and gets the same error, so nobody stays attached to a device that never opened.
4. Recognisers are not shared. A dictation uses its own recogniser instances (`dictationTranscribers`), so its text
   never appears in the meeting transcript and vice versa. The meeting WAV is unaffected by dictation.
5. `dictation.start` is **refused** with `error{code: "dictation.mic", message: "microphone access denied"}` when the
   microphone is denied. A recording can still start without the mic (system audio only).
6. When the helper is lost, every session ends. The arbiter's `helperLost` releases everything explicitly:
   `dictation.stop:helper-lost`, `recording.lost`, and `mic.unsubscribe:<who>` for each reader, so refcounts balance. The host already treats a running dictation as failed when the helper
   restarts, and the recording follows the reconnect lifecycle (`recording-lifecycle.md`).
7. A recording never runs without a file (see below).

### A recording never runs without a file

The arbiter state carries `activeAudioPath`, the file the running recording writes, and holds one invariant after
every step: **`recording` iff `activeAudioPath` is set**. Every arbiter asserts it (`assertArbiterInvariant` throws
in TS; `SessionArbiter.step` stops with `preconditionFailure` in Swift; the Rust port `arbiter_step` panics). That is what lets every `recording.busy`
name its `activeAudioPath` and every cleanup `recording.stop` carry a path: there is no pathless running recording
to stop, so the protocol has no stop without a path for it (one would be ambient authority: it stops whatever runs).

- `recording.start{audioPath}` is the only way in. A start event with no path is refused (`no-audio-path`).
- **Fail closed on the file.** When the helper cannot create the WAV for an admitted start, it tears the recording
  down itself before anything else starts: the arbiter's `recordingFileFailed{audioPath}` event returns
  `recording.lost` and `mic.unsubscribe:recording` (if the recording held the mic), the helper withdraws the
  recording's mic permit and releases its taps, answers
  `error{code: "recording.file", audioPath, fatal: false}`, and never sends `recording.started`. The arbiter is idle
  for recordings again; a dictation or warm mic running alongside is untouched (no helper restart). The event is
  ignored for any other file (`other-recording`) or when nothing records (`not-recording`).
- Host: `error{recording.file, audioPath}` for its start in flight fails that start at once (`recording-file`,
  retryable, `E_UNAVAILABLE`) instead of waiting out the ack timeout, then sends `recording.stop{audioPath}` for that
  attempt's own file like every failed start (a no-op in the helper). Any other `recording.file` (for example a WAV
  that would not finalize at stop) is only logged.
- **Watchdog, last resort.** A `recording.busy` for the host's start that names no `activeAudioPath` means the helper
  broke the invariant. The host logs it as a bug (`BUG: helper invariant broken …`), sends no stop at all, fails the
  start (`helper-busy`), and restarts the helper after the start has failed (so it never looks like a crash
  mid-recording). This also ends a healthy dictation, which is why it is not a normal path.
- Longer term the helper may mint an opaque `recordingId` at admit, so busy, stop and started carry that instead of
  the file path; it isn't needed for this rule.

### Aborting a wedged stop (N2)

`recording.stop` flushes (audio source stop, pipeline tail, recogniser finish, WAV header). If its
`recording.stopped` does not arrive within `stopAckMs`, the host does **not** restart the helper straight away, because
that would also end a healthy dictation on the shared helper. It first sends a scoped abort with a hard deadline
(`abortAckMs`, 2 s):

```json
{ "type": "recording.abort", "audioPath": "/…/recordings/0b8e.wav" }
{ "type": "recording.aborted", "audioPath": "/…/recordings/0b8e.wav" }
```

- The helper handles `recording.abort` **out of band** (`main.swift` runs it on its own task, so it never queues behind
  the wedged stop) and tears down only the recording writing `audioPath`: no recogniser finish, no tail, it never
  waits on the (possibly wedged) audio source, it closes the file, and it releases the recording's mic reader and
  permit through the arbiter (`{cmd: "recording.abort", audioPath}`: `mic.unsubscribe:recording` when the recording
  still held the mic; `other-recording` / `not-recording` otherwise). The stop that was in flight returns without
  touching anything (no late `recording.stopped`, and its samples never reach a newer recording's file).
- It always answers `recording.aborted{audioPath}`, also when nothing records that file, meaning "no writer on this
  file remains". A dictation, the warm mic and any other recording are untouched.
- Host: `recording.aborted` in time → keep the helper and post-process from the (repaired) WAV. Not in time (or the
  helper is gone) → restart the helper as before, so there is never a concurrent writer.

### The arbiter is enforcing

The helper carries out exactly the `mic.subscribe` / `mic.unsubscribe` effects the arbiter returns, on commands,
`dictation.configure`, revocations and re-grants alike, so the arbiter's `mic` list is the hub's reader list:

- `mic.subscribe:dictation` / `mic.unsubscribe:dictation` attach and detach the dictation engine's reader. The engine
  never opens or closes the mic by itself when it runs under the controller.
- `mic.subscribe:recording` permits the capture source's gated hub view (`micHub.view("recording", gated: true)`). If
  the arbiter did not subscribe the recording (mic denied at start), that view's `start` is skipped and the device is
  never opened. `mic.unsubscribe:recording` withdraws the permit and the reader.

### Warm mic

`dictation.configure{warmMic}` is an arbiter event, and the state carries a `warm` flag:

- Warm on with the mic usable subscribes the `dictation` reader (the pre-roll fills). `dictation.start` reuses it, and
  `dictation.stop` / `cancel` keep it. Warm off releases it, or at the end of the current dictation if one is running.
- A mic revoke drops the reader (and the hub's device) but keeps `warm`. After `dropAll` with no dictation running,
  the engine is told `micDropped()`, so it forgets the dead reader. When the mic is granted again, the arbiter
  resubscribes the warm reader, so the next dictation hears audio. A re-grant never resubscribes a running recording.

## Silent capture (N29)

While recording, the helper measures each raw capture channel (system audio and mic, before echo gate and mix; the
host's `audio.level` is only the mix) and reports a source that stays under a floor:

```json
{ "type": "capture.silent", "source": "system", "sinceMs": 12500 }
{ "type": "capture.audible", "source": "system" }
```

- `source`: `system` | `mic`. `sinceMs`: when the silent stretch began, recording-relative ms like other `atMs`.
- Threshold: block RMS under `floorRms` 0.0003 (≈ −70 dBFS: digital silence, a muted input, ScreenCaptureKit zeros;
  well under a quiet room) for `afterMs` 60 000. Swift `SilenceConfig.default` (`Silence.swift`, configurable per
  helper through `HelperDeps.silence`), mirrored by TS `CAPTURE_SILENCE`.
- Once per silent stretch per source; `capture.audible` when that source is above the floor again. A pause breaks a
  stretch (nothing is captured), an already reported one stays reported. A mic channel the source does not deliver
  is not tracked (no mic, or revoked: `permission.revoked` covers that). Nothing is sent outside a recording.
- Host (ADR 0025): a live per-source state, not a sticky degradation. `RecordingState.silent` / `SessionView.silent`
  and `noAudio` while it lasts. Closed stretches become `silentSpans`, kept on the meeting in
  `PipelineState.capture`. The summary step lists stretches of 60 s or more as capture gaps. Ignored: a duplicate,
  an audible with no silent before it, a source already lost. A revoke, `helper-lost`, a `to: null` fallback or
  stop closes the stretch. Electron `NoteService` and Rust `NoteRuntime`.
- Mock helper: `createMockHelper({ silence: [{ source, afterMs, audibleAfterMs?, sinceMs? }] })`, or
  `QRTZ_MOCK_SILENT="system@1500,mic@2000-4000"` (ms after `recording.start`; `-4000`: audible again then). The
  Rust `qrtz-mock-helper` takes the same value (`QRTZ_MOCK_SILENT=1` still means a mute helper there).

## Capture fallback (H8)

On `recording.start` the helper tries its audio sources in order (Core Audio process tap → ScreenCaptureKit → …).
Each one that fails to start is reported as a structured message (this used to be a free-text `log` line):

```json
{ "type": "capture.fallback", "from": "core-audio-tap", "to": "screencapturekit", "reason": "tap creation failed (OSStatus 560947818)" }
{ "type": "capture.fallback", "from": "screencapturekit", "to": null, "reason": "screen recording denied" }
```

- `from`: the source that failed (its `AudioSource.name`). `to`: the source tried next, `null` when none is left; then
  `error{code: "audio.unavailable"}` follows. `reason`: the error text.
- Sent before `recording.started`, which still follows. The host logs it as a warning and records it on the meeting
  (ADR 0025): `RecordingState.fallbacks` and later `PipelineState.capture.fallbacks` `[{from, to, reason, atMs}]`,
  capped at 8. `to: null` degrades `audio-unavailable` at once, so the following `audio.unavailable` error is a
  no-op.
- Mock helper: `createMockHelper({ captureFallback: [{ from, to, reason }] })` emits them on every `recording.start`.
  Rust `qrtz-mock-helper`: `QRTZ_MOCK_FALLBACK="screen>mic,mic>"`, where an empty `to` is `null` and is followed by
  `error{audio.unavailable}`.

## Capture degraded inside a source (N35)

A source that started but lost part of its input inside the source (today: the in-source mic of the Core Audio
tap or ScreenCaptureKit fails to start, device busy, a USB mic unplugged, an AVAudioEngine error) keeps recording
the rest and says so, instead of only a stderr line:

```json
{
  "type": "capture.degraded",
  "degraded": ["no-mic"],
  "reason": "mic unavailable, recording system audio only: device busy"
}
```

- `degraded`: one or more of the `permission.revoked` values (`RevocationDegradation`). Only `no-mic` is sent
  today. `reason`: the error text, at most 200 characters (zod `CAPTURE_DEGRADED_REASON_MAX`, the same bound the
  host keeps of a `capture.fallback` reason); Swift cuts it on whole scalars before sending.
- **Sticky until a reconnect**, like `permission.revoked` (N4): the host applies the flags and keeps them for the
  rest of the recording; a resumed recording after a helper reconnect starts from the new helper's state. There is
  no `capture.restored` for now (later addition: a source getting its mic back mid-recording).
- **`no-mic` counts as a lost mic, like a revoke:** it closes any open mic silence stretch (N29) and the mic counts
  as not delivering for `noAudio`, so the system channel going silent afterwards is "no audio". A later
  `capture.silent{mic}` (or `capture.audible{mic}`) is ignored.
- Helper: a source reports through `AudioSource.start(onBlock:onDegraded:)` (`CoreAudioTapSource` and
  `ScreenCaptureKitSource` when their mic start throws; this used to be a stderr-only line). The controller holds
  reports made during `start` and sends them after `recording.started`; it sends each flag once per recording, only
  while it is recording (nothing after its stop or abort), and drops the reports of a source that then failed to
  start (its `capture.fallback` covers it).
- `capture.fallback` does not fit this: on the host, its `to: null` means no source is left (`audio-unavailable`).
- Host (ADR 0028, Electron `NoteService` and Rust `NoteRuntime`):
  - Only the flags not already set are applied, and they are remembered in `RecordingState.deviceLost` /
    `SessionView.deviceLost`.
  - `helper-restored` clears them. A later revoke of the same flag takes it over, so it stays.
  - Ignored outside `recording`: before the ack, or while reconnecting.
  - The meeting gets `PipelineState.capture.deviceFailures [{degraded, reason, atMs}]` (at most 8). A revoke never
    writes one.
- Mock helpers: TS `createMockHelper({ micFails: { afterMs, reason? } })`, and `QRTZ_MOCK_MIC_FAILS=1500` for both
  the TS mock and the Rust `qrtz-mock-helper`. Each sends `capture.degraded{no-mic}` after `recording.started`, once
  per recording, never after a stop. Scripted mic silence after it is not sent.

## Permission revocation (H5)

While a recording or dictation is running, or the warm mic is on (even with nothing running), the helper re-reads TCC every `permissionPollMs` (2 s by default) and diffs
it against the last snapshot. A granted → denied transition emits:

```json
{
  "type": "permission.revoked",
  "permission": "microphone",
  "atMs": 1500,
  "degraded": ["no-mic"]
}
```

`degraded` (N4) is the helper arbiter's `degrade:*` effects for this revocation, worked out from the helper's own TCC
state: `[]` when no recording is running. Values: `ax-unavailable`, `audio-unavailable`, `mic-only`, `no-mic`. It is
optional only so lines from an older helper still parse.

`atMs` is on the helper clock (recording-relative while recording). Each revocation fires once. Re-granting clears the
helper's denied set, so the next session can use the source again. It never clears a degradation within the current recording.

Effects (the arbiter's `revoked` events):

| Revoked            | Recording (host adds a degradation, live)                                                            | Dictation                                                                                                                                    |
| ------------------ | ---------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `microphone`       | `no-mic` (system audio still captured), or `audio-unavailable` if screen recording is already denied | The helper ends it with `dictation.stopped{reason: "permission-revoked", text: <heard so far>}`, and the mic tap is dropped for every reader |
| `screen-recording` | `mic-only`, or `audio-unavailable` if the mic is already denied                                      | Unaffected                                                                                                                                   |
| `accessibility`    | `ax-unavailable`                                                                                     | Keeps going. Insertion falls back to paste or clipboard                                                                                      |

On the host side:

- `NoteService` (and the Rust `NoteRuntime`) applies `permission.revoked.degraded` to the session's `degraded[]` as is.
  Only when the field is missing (older helper) does it derive the list with `degradationsForRevocation` from its own
  permission snapshot, which may still be `unknown` (scenario `N4-revoked-carries-degraded`), so the recording keeps running with badges. It also updates the permission state. The note view never
  replaces a running recording with onboarding.
- `TalkService` treats an unsolicited `dictation.stopped{reason}` as a graceful stop. It cleans up and inserts the text
  heard so far, keeps the history entry, then shows `microphone access was turned off, so dictation stopped`.

## Tests

- Shared vectors (16 cases): `packages/core/src/helper/arbiter.test.ts`, `ArbiterTests.testSharedVectors`. Edge vectors
  (`spec/vectors/helper/arbiter-edges.json`) are replayed by `arbiter-model.test.ts` (an independent model, run
  exhaustively over reachable states, including warm/configure) and by `ArbiterTests.testEdgeVectors`. A second
  declarative model (`arbiter-declarative.test.ts`) checks the reader set. Both models track the recording's file
  and assert the file invariant on every step, with path-less starts and `recordingFileFailed` for the active and
  another file among the events.
- Swift: `ArbiterTests`. Covers `MicHub` refcount and fan-out, mux through `HelperController`, mic revoked mid-mux, and
  accessibility revoked during dictation, the warm mic after a revoke and re-grant, busy errors and a mismatched stop
  path, the gated recording view with the mic denied, an unsubscribe racing the device start, a failed device start
  with a joiner (rethrown, detached), and the busy unlock paths (`dictation.busy{id, activeId}` then
  `cancel{activeId}`; `recording.busy{audioPath, activeAudioPath}` then `stop{activeAudioPath}`), the file rule
  (`testArbiterFileRuleAndInvariant`) and a WAV that can't be created (`testRecordingFileFailureTearsTheRecordingDown`:
  `error{recording.file, audioPath}`, no `started`, mic permit withdrawn, dictation kept, the next start admitted).
- Mock helper (child protocol): `packages/core/src/helper/mock-concurrency.test.ts` (`failRecordingFile` injects the
  file failure).
- Host: `note-service.test.ts` (live `mic-only` → `audio-unavailable`, `no-mic`, `recording.busy`, an orphaned
  recording A stopped by path so the retry starts, busy for another file ignored, `recording.file` failing the start
  fast without a restart, the pathless-busy watchdog restart) and `talk-service.test.ts`
  (graceful stop, then refusal, `dictation.busy`, a helper still running d1 after a host reload no longer locks
  dictation out, busy for another id / a duplicate start ignored).
- Lifecycle scenarios (TS oracle and Rust `NoteRuntime`): `spec/scenarios/lifecycle/F5-recording-file.json`,
  `F5-busy-no-active-file.json`, `F6-stop-ack-abort.json` (abort acked, no restart) and `F6-stop-ack-timeout.json`
  (abort not acked either, then restart).
- N2: `AbortTests` (Swift, a stop wedged in the audio source), `mock-concurrency.test.ts`, `note-service.test.ts`
  (a dictation survives an acked abort; a wedged abort restarts).
- e2e: `e2e/permissions.spec.ts`. Uses `QRTZ_MOCK_REVOKE="screen-recording@800,microphone@2000"` to revoke TCC that
  many ms after the first capture.
- On a real Mac: see `mac-verify.md` (revoking in System Settings mid-capture, and two readers on one input device).

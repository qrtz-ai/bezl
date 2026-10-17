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

## Permission revocation (H5)

While a recording or dictation is running, or the warm mic is on (even with nothing running), the helper re-reads TCC every `permissionPollMs` (2 s by default) and diffs
it against the last snapshot. A granted → denied transition emits:

```json
{ "type": "permission.revoked", "permission": "microphone", "atMs": 1500 }
```

`atMs` is on the helper clock (recording-relative while recording). Each revocation fires once. Re-granting clears the
helper's denied set, so the next session can use the source again. It never clears a degradation within the current recording.

Effects (the arbiter's `revoked` events):

| Revoked            | Recording (host adds a degradation, live)                                                            | Dictation                                                                                                                                    |
| ------------------ | ---------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `microphone`       | `no-mic` (system audio still captured), or `audio-unavailable` if screen recording is already denied | The helper ends it with `dictation.stopped{reason: "permission-revoked", text: <heard so far>}`, and the mic tap is dropped for every reader |
| `screen-recording` | `mic-only`, or `audio-unavailable` if the mic is already denied                                      | Unaffected                                                                                                                                   |
| `accessibility`    | `ax-unavailable`                                                                                     | Keeps going. Insertion falls back to paste or clipboard                                                                                      |

On the host side:

- `NoteService` maps `permission.revoked` through `degradationsForRevocation` (shared with the arbiter) into the session's
  `degraded[]`, so the recording keeps running with badges. It also updates the permission state. The note view never
  replaces a running recording with onboarding.
- `TalkService` treats an unsolicited `dictation.stopped{reason}` as a graceful stop. It cleans up and inserts the text
  heard so far, keeps the history entry, then shows `microphone access was turned off, so dictation stopped`.

## Tests

- Shared vectors (15 cases): `packages/core/src/helper/arbiter.test.ts`, `ArbiterTests.testSharedVectors`. Edge vectors
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
- Lifecycle scenarios (TS oracle and Rust `NoteRuntime`): `spec/scenarios/lifecycle/F5-recording-file.json` and
  `F5-busy-no-active-file.json`.
- e2e: `e2e/permissions.spec.ts`. Uses `QRTZ_MOCK_REVOKE="screen-recording@800,microphone@2000"` to revoke TCC that
  many ms after the first capture.
- On a real Mac: see `mac-verify.md` (revoking in System Settings mid-capture, and two readers on one input device).

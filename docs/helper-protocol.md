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
   **ignored** (it is not queued). The helper logs `"<cmd> while already-recording|already-dictating; ignored"`. Stops
   and cancels when nothing is running are no-ops (`not-recording` / `not-dictating`). `dictation.stop` and `cancel`
   with a stale id are no-ops too.
3. Shared mic tap (`MicHub`). The physical mic (one `AVAudioEngine` input tap) starts on the first reader and stops on
   the last. Readers are `recording` (the capture source's mic leg, mixed with system audio and echo-gated) and `dictation`.
   Every reader gets every block. When both are running, the helper logs `mux: recording and dictation share the mic tap`.
   The warm mic keeps a `dictation` reader subscribed between dictations. The arbiter models only active sessions.
4. Recognisers are not shared. A dictation uses its own recogniser instances (`dictationTranscribers`), so its text
   never appears in the meeting transcript and vice versa. The meeting WAV is unaffected by dictation.
5. `dictation.start` is **refused** with `error{code: "dictation.mic", message: "microphone access denied"}` when the
   microphone is denied. A recording can still start without the mic (system audio only).
6. When the helper is lost, every session ends. The host already treats a running dictation as failed when the helper
   restarts, and the recording follows the reconnect lifecycle (`recording-lifecycle.md`).

## Permission revocation (H5)

While a recording or dictation is running, the helper re-reads TCC every `permissionPollMs` (2 s by default) and diffs
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

- Shared vectors (10 cases, 49 steps): `packages/core/src/helper/arbiter.test.ts`, `ArbiterTests.testSharedVectors`.
- Swift: `ArbiterTests`. Covers `MicHub` refcount and fan-out, mux through `HelperController`, mic revoked mid-mux, and
  accessibility revoked during dictation.
- Mock helper (child protocol): `packages/core/src/helper/mock-concurrency.test.ts`.
- Host: `note-service.test.ts` (live `mic-only` → `audio-unavailable`, `no-mic`) and `talk-service.test.ts` (graceful stop, then refusal).
- e2e: `e2e/permissions.spec.ts`. Uses `QRTZ_MOCK_REVOKE="screen-recording@800,microphone@2000"` to revoke TCC that
  many ms after the first capture.
- On a real Mac: see `mac-verify.md` (revoking in System Settings mid-capture, and two readers on one input device).

import { z } from 'zod';

/**
 * qrtz speech module protocol (ADR 0046). Model-agnostic: nothing here names an engine or a model. The host spawns
 * the module with inherited fds (2 log, 3 control in, 4 results out, 5 mic PCM, 6 system PCM, 7.. one read-only fd per
 * model file; nothing else survives the spawn) and
 * validates every results line against this schema plus the bounds in SPEECH_LIMITS (outputs are untrusted).
 * Rendered to contracts/speech-protocol.json for the Rust host (crates/qrtz-speech).
 */
export const SPEECH_PROTOCOL_VERSION = 1;

/** Bounds the host enforces on module output (ADR 0046 §3.5). Mirrored in crates/qrtz-speech/src/validate.rs. */
export const SPEECH_LIMITS = {
  lineBytes: 65_536,
  wordsPerEvent: 64,
  wordChars: 64,
  utteranceChars: 2_000,
  eventsPerSecond: 200,
  speakersPerChannel: 8,
  idChars: 64,
  logChars: 500,
  /** A word or speaker span may end at most this far past the audio written so far. */
  aheadMs: 1_000,
  /** Protocol violations in one session before the host fails it. */
  violationsToFail: 3,
} as const;

export const SpeechChannel = z.enum(['mic', 'system']);
export type SpeechChannel = z.infer<typeof SpeechChannel>;
/** Per-channel speaker id; ids are never shared across channels. */
export const SpeechSpeakerId = z.string().regex(/^(mic|system):[0-7]$/);
export const SpeechSessionKind = z.enum(['recording', 'dictation']);
/** User-facing trade-off, mapped to engine presets inside the module. */
export const SpeakerMode = z.enum(['fast', 'accurate']);
export type SpeakerMode = z.infer<typeof SpeakerMode>;
const Id = z.string().min(1).max(SPEECH_LIMITS.idChars);
const Ms = z.number().int().min(0);
/** BCP-47 shape; the host also checks an allowlist (SPEECH_LANGUAGES). */
export const Bcp47 = z.string().regex(/^[a-z]{2,3}(-[A-Z]{2})?$/);
export const SPEECH_LANGUAGES = [
  'en',
  'en-US',
  'en-GB',
  'en-AU',
  'en-CA',
  'en-IN',
] as const;

export const SpeechWord = z.object({
  text: z.string().min(1).max(SPEECH_LIMITS.wordChars),
  startMs: Ms,
  endMs: Ms,
});

export const HostToSpeech = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('start'),
    sessionId: Id,
    kind: SpeechSessionKind,
    channels: z.array(SpeechChannel).min(1).max(2),
    sampleRate: z.literal(16_000),
    languageHint: Bcp47.optional(),
    speakerMode: SpeakerMode,
    /**
     * N56: each model file is handed in as its own read-only fd (opened by the host after its hash check), never a
     * path or a dir fd. `path` is the manifest path, for the engine's own lookup.
     */
    files: z
      .array(
        z.object({
          path: z.string().min(1).max(255),
          fd: z.number().int().min(7).max(1024),
          size: z.number().int().min(0),
        }),
      )
      .max(512),
  }),
  /** Flush everything and answer `finished`. The host closes the audio fds first. */
  z.object({ type: z.literal('finish'), sessionId: Id }),
  z.object({ type: z.literal('cancel'), sessionId: Id }),
  z.object({ type: z.literal('shutdown') }),
]);
export type HostToSpeech = z.infer<typeof HostToSpeech>;

export const SpeechToHost = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('hello'),
    protocol: z.number().int(),
    moduleVersion: z.string().regex(/^\d+\.\d+\.\d+$/),
    languages: z.array(Bcp47).max(32),
    maxSpeakersPerChannel: z
      .number()
      .int()
      .min(0)
      .max(SPEECH_LIMITS.speakersPerChannel),
  }),
  z.object({
    type: z.literal('status'),
    phase: z.enum(['loading', 'ready', 'failed']),
    error: z.string().max(SPEECH_LIMITS.logChars).optional(),
  }),
  z.object({
    /** A partial replaces the previous partial of the utterance; a final closes it. */
    type: z.literal('words'),
    utteranceId: Id,
    channel: SpeechChannel.optional(),
    final: z.boolean(),
    language: Bcp47,
    words: z.array(SpeechWord).max(SPEECH_LIMITS.wordsPerEvent),
  }),
  z.object({
    type: z.literal('speaker'),
    channel: SpeechChannel,
    speaker: SpeechSpeakerId,
    startMs: Ms,
    endMs: Ms,
  }),
  z.object({ type: z.literal('finished'), sessionId: Id, durationMs: Ms }),
  z.object({
    type: z.literal('log'),
    level: z.enum(['debug', 'info', 'warn', 'error']),
    msg: z.string().max(SPEECH_LIMITS.logChars),
  }),
]);
export type SpeechToHost = z.infer<typeof SpeechToHost>;

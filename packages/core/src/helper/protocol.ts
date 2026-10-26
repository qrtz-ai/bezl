import { z } from 'zod';
import { OsPermission } from '../capabilities';
import { AsrEvent, MeetingPlatform } from '../note/types';

/** The recording degradations a TCC revocation can cause (subset of the lifecycle `Degradation`). */
export const RevocationDegradation = z.enum([
  'ax-unavailable',
  'audio-unavailable',
  'mic-only',
  'no-mic',
]);
export type RevocationDegradation = z.infer<typeof RevocationDegradation>;

/** N35: `capture.degraded.reason` bound, the same 200 as the host keeps of a `capture.fallback` reason. */
export const CAPTURE_DEGRADED_REASON_MAX = 200;

/** N29: a raw capture channel, as `capture.silent` / `capture.audible` name it. */
export const CaptureSource = z.enum(['system', 'mic']);
export type CaptureSource = z.infer<typeof CaptureSource>;

/**
 * N29: when the helper calls a source silent. Mirrors Swift `SilenceConfig.default` (Silence.swift): block RMS
 * under `floorRms` (linear, ≈ −70 dBFS) for `afterMs`. Configurable per helper via `HelperDeps.silence`.
 */
export const CAPTURE_SILENCE = { floorRms: 0.0003, afterMs: 60_000 } as const;

/**
 * Wire protocol between the app (host) and the native Swift helper (`qrtz-helper`).
 * Transport: newline-delimited JSON over stdio. One message per line.
 * This file is the source of truth; the Swift side mirrors it (native/qrtz-helper/Sources/QrtzShared/Protocol.swift).
 */
export const HELPER_PROTOCOL_VERSION = 1;

/**
 * ADR 0075: on-device summaries through the helper (Apple Intelligence, FoundationModels). Bounds on what the
 * helper may send back; the Swift side caps to the same numbers and the Rust host re-checks them.
 */
export const SUMMARIZE_LIMITS = {
  /** A prompt the host builds (title, notes, one fenced transcript part). */
  promptMax: 40_000,
  summaryMax: 4_000,
  actionItemsMax: 12,
  itemTextMax: 400,
  nameMax: 80,
  qaMax: 8,
  questionMax: 300,
  answerMax: 600,
  messageMax: 300,
} as const;
const SL = SUMMARIZE_LIMITS;
/** Why on-device summaries can't run (`SystemLanguageModel.Availability`, plus the build/OS cases). */
export const SummarizeUnavailableReason = z.enum([
  'apple-intelligence-off',
  'device-not-eligible',
  'model-not-ready',
  'os-unsupported',
  'unsupported',
]);
export type SummarizeUnavailableReason = z.infer<
  typeof SummarizeUnavailableReason
>;
const SummarizeId = z.string().min(1).max(64);

const PermissionState = z.enum([
  'granted',
  'denied',
  'not-determined',
  'unsupported',
]);

/** ADR 0217: bounds on the calendar messages (the helper caps, the host re-checks). */
export const CALENDAR_LIMITS = {
  titleMax: 200,
  urlMax: 500,
  idMax: 512,
  calendarsMax: 200,
  eventsMax: 2000,
  /** The widest window one `calendar.read` may ask for: 15 days. */
  windowMaxMs: 15 * 24 * 3_600_000,
} as const;
const CalendarReqId = z.string().min(1).max(64);
const CalendarText = z.string().min(1).max(CALENDAR_LIMITS.idMax);
export const CalendarAccessState = z.enum([
  'not-determined',
  'granted',
  'denied',
  'restricted',
  'write-only',
  'unsupported',
]);
export type CalendarAccessState = z.infer<typeof CalendarAccessState>;

export const HelperToHost = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('hello'),
    protocol: z.number().int(),
    helperVersion: z.string(),
    os: z.string(),
    features: z.array(z.string()),
  }),
  z.object({ type: z.literal('heartbeat'), ts: z.number() }),
  z.object({ type: z.literal('pong'), id: z.string() }),
  z.object({
    type: z.literal('permissions.status'),
    status: z.record(OsPermission, PermissionState),
  }),
  z.object({
    type: z.literal('log'),
    level: z.enum(['debug', 'info', 'warn', 'error']),
    msg: z.string(),
  }),
  /** Experimental Phonon-2.5: echo's second recogniser's load state, on every change after `echo.phonon`. */
  z.object({
    type: z.literal('echo.phonon.state'),
    state: z.enum(['off', 'loading', 'ready', 'error']),
    error: z.string().max(200).optional(),
  }),
  z.object({
    type: z.literal('error'),
    code: z.string(),
    message: z.string(),
    fatal: z.boolean().default(false),
    /**
     * Set on `dictation.busy`: `id` is the dictation.start that was ignored, `activeId` the dictation that
     * still owns the helper. The host acts only when `id` is its start in flight.
     */
    id: z.string().optional(),
    activeId: z.string().optional(),
    /**
     * Set on `recording.busy`: `audioPath` is the recording.start that was ignored, `activeAudioPath` the
     * file the running recording writes. The host acts only when `audioPath` is its start in flight.
     * `activeAudioPath` is always set on recording.busy (a running recording always has a file); a busy without
     * it means the helper broke that invariant, and the host restarts the helper (last-resort watchdog).
     * Also set on `recording.file` when the file of recording.start{audioPath} could not be created: the helper
     * has already torn that recording down (no recording.started follows), so the host fails the start at once.
     */
    audioPath: z.string().optional(),
    activeAudioPath: z.string().optional(),
    /** N195 (ADR 0224), `dictation.mic` only: why the mic didn't open, when the helper can tell. */
    reason: z.enum(['denied', 'no-device', 'failed']).optional(),
  }),
  // --- note (AX + audio + ASR) ---
  z.object({
    type: z.literal('meeting.detected'),
    key: z.string(),
    title: z.string(),
    platform: MeetingPlatform,
    app: z.string(),
  }),
  z.object({ type: z.literal('meeting.ended'), key: z.string() }),
  z.object({ type: z.literal('ax.participants'), names: z.array(z.string()) }),
  z.object({
    type: z.literal('ax.activeSpeaker'),
    atMs: z.number(),
    name: z.string(),
  }),
  z.object({
    type: z.literal('ax.caption'),
    atMs: z.number(),
    speaker: z.string().optional(),
    text: z.string(),
  }),
  z.object({
    type: z.literal('ax.chat'),
    atMs: z.number(),
    author: z.string(),
    text: z.string(),
  }),
  z.object({ type: z.literal('audio.level'), rms: z.number().min(0).max(1) }),
  z.object({
    /**
     * N29: while recording, `source` has been under the silence floor since `sinceMs` (recording-relative ms, like
     * other atMs fields) for `CAPTURE_SILENCE.afterMs`. Once per silent stretch; `capture.audible` ends it.
     */
    type: z.literal('capture.silent'),
    source: CaptureSource,
    sinceMs: z.number().min(0),
  }),
  z.object({
    /** N29: `source` is above the floor again after a `capture.silent`. */
    type: z.literal('capture.audible'),
    source: CaptureSource,
  }),
  z.object({
    /**
     * ADR 0079: one speaker's turn on one raw channel from the helper's on-device diarizer (nano only), in
     * recording-relative ms like other atMs fields. `speaker` is that channel's slot (the host names it `mic:N` /
     * `system:N`); slots are never merged across channels.
     */
    type: z.literal('diarization.segment'),
    source: CaptureSource,
    speaker: z.number().int().min(0).max(7),
    startMs: z.number().min(0),
    endMs: z.number().min(0),
  }),
  z.object({
    /**
     * N126 (ADR 0082): `source` gets no more speaker turns this recording from `atMs` (recording-relative) on.
     * `backlog`: its diarizer fell too far behind and stood down (the turns up to there were sent first);
     * `model-unavailable`: its model didn't load. Once per channel per recording; lines after it keep no speaker.
     */
    type: z.literal('diarization.unavailable'),
    source: CaptureSource,
    atMs: z.number().min(0),
    reason: z.enum(['backlog', 'model-unavailable']),
  }),
  z.object({
    /**
     * H8: a capture source failed to start and the helper moves on to the next one (Core Audio process tap →
     * ScreenCaptureKit → …). `to` is the source tried next; null when none is left (`error{audio.unavailable}`
     * follows). Informational: the host logs it; recording.started still follows.
     */
    type: z.literal('capture.fallback'),
    from: z.string().min(1),
    to: z.string().min(1).nullable(),
    reason: z.string(),
  }),
  z.object({
    /**
     * N35: a capture source started but lost part of its input inside the source (the in-source mic of the tap or
     * SCK failed: device busy, unplugged, engine error); it keeps recording the rest. Sticky until a reconnect, like
     * `permission.revoked` (N4); there is no `capture.restored` yet. Today only `no-mic` is sent. `reason` is the
     * error text, at most CAPTURE_DEGRADED_REASON_MAX characters (the host's capture-fallback reason bound).
     */
    type: z.literal('capture.degraded'),
    degraded: z.array(RevocationDegradation).min(1),
    reason: z.string().max(CAPTURE_DEGRADED_REASON_MAX),
  }),
  z.object({ type: z.literal('asr'), event: AsrEvent }),
  z.object({ type: z.literal('recording.started'), audioPath: z.string() }),
  z.object({
    type: z.literal('recording.stopped'),
    audioPath: z.string(),
    durationMs: z.number(),
  }),
  /**
   * Answer to recording.abort{audioPath}: no writer on that file remains (the recording was torn down without a
   * flush, or nothing was recording it). Sent even when there was nothing to abort.
   */
  z.object({ type: z.literal('recording.aborted'), audioPath: z.string() }),
  // --- talk (dictation: mic only) ---
  /** Push-to-talk key from the helper's native key monitor (Electron shortcuts have no key-up). */
  // ADR 0216: `ctrl`: the press was ctrl+fn (its transcript goes to ctrl do, never inserted).
  z.object({
    type: z.literal('hotkey'),
    action: z.enum(['down', 'up']),
    ctrl: z.boolean().optional(),
    // R2 (ADR 0216): a release the helper made up at sleep/wake (N196); a ctrl+fn command it ends runs nothing.
    sleep: z.boolean().optional(),
  }),
  z.object({
    type: z.literal('dictation.started'),
    id: z.string(),
    /** Audio from before the key press that was replayed from the warm-mic buffer. */
    preRollMs: z.number(),
  }),
  z.object({
    type: z.literal('dictation.level'),
    rms: z.number().min(0).max(1),
  }),
  /** On-device streaming ASR for the dictation. */
  z.object({
    type: z.literal('dictation.asr'),
    id: z.string(),
    event: AsrEvent,
  }),
  /** 16 kHz mono PCM16 (base64) for a cloud streaming engine; only sent when streamAudio is on. */
  z.object({
    type: z.literal('dictation.audio'),
    id: z.string(),
    seq: z.number().int(),
    pcm16: z.string(),
  }),
  z.object({
    type: z.literal('dictation.stopped'),
    id: z.string(),
    /** Final on-device text ('' when no recogniser ran). */
    text: z.string(),
    durationMs: z.number(),
    /**
     * Set when the helper ended the dictation on its own (no dictation.stop from the host):
     * `permission-revoked` (microphone access withdrawn mid-dictation) or `mic-lost` (the input device went away).
     */
    reason: z.enum(['permission-revoked', 'mic-lost']).optional(),
    /** The recogniser that heard it, e.g. `parakeet-tdt-v2` (echo), `nemotron-560ms`, `apple-speechanalyzer`. Absent when none ran (or an older helper). */
    asr: z.string().min(1).optional(),
    /** Segmented batch recogniser (echo on Parakeet): segments decoded, the tail included. */
    segments: z.number().int().nonnegative().optional(),
    /** Segmented batch recogniser: the release's decode of the last segment (ms). Absent when the tail had no speech to decode. */
    tailDecodeMs: z.number().nonnegative().optional(),
    /** ADR 0132: the release tail decode's confidence (0..1, 3 places). Absent when the recogniser reported none (or FluidAudio's placeholder) or no tail was decoded. */
    tailConf: z.number().min(0).max(1).optional(),
    /** ADR 0131/0132: true when the helper's tail confidence floor dropped the release tail's words; absent otherwise. */
    tailDropped: z.boolean().optional(),
    /** Experimental Phonon-2.5: the engine that decoded it (`parakeet-tdt-v2`, `phonon-2.5-en`, or both joined with
     * `+`). Absent unless echo's experimental router ran. */
    engine: z.string().min(1).optional(),
    /** Experimental Phonon-2.5: why Parakeet was used while Phonon was on. Absent otherwise. */
    engineFallback: z
      .enum(['not-loaded', 'too-short', 'too-long', 'error'])
      .optional(),
  }),
  z.object({
    /**
     * A TCC permission went from granted to denied while capturing or dictating (polled by the helper).
     * The host degrades the recording live and the helper ends a dictation itself (docs/helper-protocol.md).
     */
    type: z.literal('permission.revoked'),
    permission: OsPermission,
    /** ms on the helper clock (recording-relative while recording, like other atMs fields). */
    atMs: z.number(),
    /**
     * N4: the recording degradations the helper's arbiter derived from its own TCC state (its `degrade:*`
     * effects; `[]` when it is not recording). The host applies exactly these and derives them from its own
     * permission snapshot only when an older helper omits the field.
     */
    degraded: z.array(RevocationDegradation).optional(),
  }),
  /** N55: the audio write ends named by speech.attach were received and checked (FIFO, write-only, exact count). */
  z.object({
    type: z.literal('speech.attached'),
    sessionId: z.string().max(256),
  }),
  /** N55: refused; every received fd was closed and nothing is written. */
  z.object({
    type: z.literal('speech.attach.rejected'),
    sessionId: z.string().max(256),
    reason: z.enum([
      'no-fds',
      'count',
      'not-fifo',
      'not-write-only',
      'session-mismatch',
      'unsupported',
    ]),
  }),
  /** ADR 0075: the guided-generation result of one `summarize` request (schema-capped data only). */
  z.object({
    type: z.literal('summarize.result'),
    id: SummarizeId,
    summary: z.string().max(SL.summaryMax),
    actionItems: z
      .array(
        z.object({
          text: z.string().max(SL.itemTextMax),
          owner: z.string().max(SL.nameMax).optional(),
        }),
      )
      .max(SL.actionItemsMax),
    qa: z
      .array(
        z.object({
          question: z.string().max(SL.questionMax),
          answer: z.string().max(SL.answerMax),
          askedBy: z.string().max(SL.nameMax).optional(),
        }),
      )
      .max(SL.qaMax),
  }),
  /**
   * ADR 0075: a `summarize` request failed. `unavailable` carries the reason; `context` = the prompt is over the
   * model's context window (the host splits the part and asks again); `guardrail` = the model refused.
   */
  z.object({
    type: z.literal('summarize.error'),
    id: SummarizeId,
    code: z.enum([
      'unavailable',
      'context',
      'guardrail',
      'busy',
      'cancelled',
      'failed',
    ]),
    reason: SummarizeUnavailableReason.optional(),
    message: z.string().max(SL.messageMax),
  }),
  /** ADR 0075: answer to `summarize.check`. */
  z.object({
    type: z.literal('summarize.status'),
    id: SummarizeId,
    available: z.boolean(),
    reason: SummarizeUnavailableReason.optional(),
    contextTokens: z.number().int().min(0).optional(),
  }),
  /**
   * ADR 0217: nano's Apple Calendar access (EventKit, read-only). Answers `calendar.check` / `calendar.request`
   * with their id; sent without an id when the state changes on its own (System Settings). `write-only` (macOS 14+)
   * can't read events, so nano treats it like `denied`.
   */
  z.object({
    type: z.literal('calendar.access'),
    id: CalendarReqId.optional(),
    state: CalendarAccessState,
  }),
  /**
   * ADR 0217: answer to `calendar.read`: every calendar the user has, and the events in the window with only the
   * fields nano keeps (no notes, location, attendee names or addresses). Titles are capped and cleaned by the
   * helper and again by the host; `meetingUrl` is an https video-call link or absent.
   */
  z.object({
    type: z.literal('calendar.events'),
    id: CalendarReqId,
    calendars: z
      .array(z.object({ id: CalendarText, title: CalendarText }))
      .max(CALENDAR_LIMITS.calendarsMax),
    events: z
      .array(
        z.object({
          id: CalendarText,
          calendarId: CalendarText,
          title: z.string().max(CALENDAR_LIMITS.titleMax),
          startMs: z.number().int(),
          endMs: z.number().int(),
          allDay: z.boolean(),
          attendees: z.number().int().min(0).max(10_000),
          meetingUrl: z.string().max(CALENDAR_LIMITS.urlMax).optional(),
        }),
      )
      .max(CALENDAR_LIMITS.eventsMax),
  }),
  /** ADR 0217: EKEventStoreChanged: the calendar database changed; the host reads the window again. */
  z.object({ type: z.literal('calendar.changed') }),
  z.object({
    type: z.literal('insert.result'),
    id: z.string(),
    method: z.enum(['ax-selected-text', 'ax-value', 'paste', 'none']),
    ok: z.boolean(),
    app: z.string().optional(),
    error: z.string().optional(),
  }),
]);
export type HelperToHost = z.infer<typeof HelperToHost>;

/** ADR 0221: the notch surface's wire types (mirrored by native/qrtz-helper/Sources/QrtzShared/Notch.swift and
 * the @qrtz/ui NotchSurface mock). No clickable actions in v1. */
export const NOTCH_PRIORITIES = ['system', 'ctrl', 'module'] as const;
export const NOTCH_ICONS = [
  'check',
  'alert',
  'info',
  'mic',
  'clock',
  'download',
  'bolt',
  'calendar',
] as const;
export const NOTCH_LIMITS = {
  labelMax: 60,
  statusMax: 120,
  ownerMax: 64,
  sourceMax: 40,
} as const;
const NotchOwner = z
  .string()
  .min(1)
  .max(NOTCH_LIMITS.ownerMax)
  .regex(/^[a-z0-9._-]+$/);
export const NotchContent = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('level'), busy: z.boolean() }),
  z.object({
    kind: z.literal('status'),
    text: z.string().min(1).max(NOTCH_LIMITS.statusMax),
    tone: z.enum(['normal', 'error']),
  }),
  z.object({
    kind: z.literal('progress'),
    label: z.string().min(1).max(NOTCH_LIMITS.labelMax),
    value: z.number().min(0).max(1).optional(),
  }),
  z.object({
    kind: z.literal('icon'),
    icon: z.enum(NOTCH_ICONS),
    label: z.string().min(1).max(NOTCH_LIMITS.labelMax),
  }),
]);
export type NotchContent = z.infer<typeof NotchContent>;

export const HostToHelper = z.discriminatedUnion('type', [
  z.object({ type: z.literal('ping'), id: z.string() }),
  z.object({ type: z.literal('permissions.check') }),
  z.object({
    type: z.literal('permissions.request'),
    permission: OsPermission,
  }),
  z.object({ type: z.literal('shutdown') }),
  z.object({ type: z.literal('recording.start'), audioPath: z.string() }),
  z.object({
    type: z.literal('recording.stop'),
    /** Only stop the recording writing this file; the helper ignores a stop for any other path. */
    audioPath: z.string().optional(),
  }),
  /**
   * N2: the host gave up on recording.stop's ack. Tear down the recording writing `audioPath` now, without the
   * flush (no recogniser finish, no tail), close its file and answer recording.aborted{audioPath}. Scoped: any
   * other recording, and every dictation, is untouched. Handled out of band, never queued behind a wedged stop.
   */
  z.object({ type: z.literal('recording.abort'), audioPath: z.string() }),
  /** Privacy pause: the helper writes silence and transcribes nothing until resume. */
  z.object({ type: z.literal('recording.pause') }),
  z.object({ type: z.literal('recording.resume') }),
  /** ADR 0079 / N121: the speech models were just installed; load each once (one at a time) so the OS compiles
   * them now, not at the first recording. Ignored while a recording runs. */
  z.object({ type: z.literal('models.warm') }),
  /** Experimental Phonon-2.5 (`echo.phononExperimental`, default off): load (true) or unload (false) echo's second
   * recogniser. Parakeet is unaffected. */
  z.object({ type: z.literal('echo.phonon'), enabled: z.boolean() }),
  /** Dictation settings: warm mic + pre-roll, cloud audio streaming, native push-to-talk key ('' = none). */
  z.object({
    type: z.literal('dictation.configure'),
    warmMic: z.boolean(),
    preRollMs: z.number().int().min(0).max(2000),
    streamAudio: z.boolean(),
    pushKey: z.string(),
    /** With pushKey 'fn': swallow the Fn/globe events so macOS doesn't also run its globe action (switch input
     * source, emoji). Absent = on (`talk.blockGlobeKey`). */
    consumeFn: z.boolean().optional(),
    /** ADR 0092: `talk.showPill` for the helper's native pill. Absent = shown. */
    pill: z.boolean().optional(),
    autoInsert: z.boolean().optional(),
    echoGate: z.boolean().optional(),
    /** ADR 0221, temporary for tuning: `talk.pillMotion` (absent = the helper's snappy defaults). */
    pillMotion: z.record(z.string(), z.number()).optional(),
  }),
  /** ADR 0092: a failure or notice only the host knows (an older dictation's failed insert, the clipboard
   * fallback: info "copied to clipboard"), shown on the helper's native pill. Lowercase text. */
  z.object({
    type: z.literal('pill.flash'),
    kind: z.enum(['error', 'info']),
    message: z.string().min(1).max(200),
  }),
  /** ADR 0221: the notch surface. Owner and priority are host-assigned (never module-supplied); module text is
   * sanitized, capped and attributed (`source`, the module's registered display name) by the host first. The
   * helper drops host `system` shows: that priority is its own dictation. */
  z.object({
    type: z.literal('notch.show'),
    owner: NotchOwner,
    priority: z.enum(NOTCH_PRIORITIES),
    content: NotchContent,
    source: z.string().min(1).max(NOTCH_LIMITS.sourceMax).optional(),
  }),
  /** Only the current holder's update takes effect. */
  z.object({
    type: z.literal('notch.update'),
    owner: NotchOwner,
    content: NotchContent,
  }),
  z.object({ type: z.literal('notch.dismiss'), owner: NotchOwner }),
  z.object({ type: z.literal('dictation.start'), id: z.string() }),
  z.object({ type: z.literal('dictation.stop'), id: z.string() }),
  z.object({ type: z.literal('dictation.cancel'), id: z.string() }),
  /**
   * ADR 0046 §1 / N55: the speech module's audio write ends, one per channel in this order, arrive as one
   * SCM_RIGHTS message (payload = sessionId) on the speech socket (fd QRTZ_SPEECH_FD), sent just before this line.
   * `target` names the capture that feeds them: a dictation by id or a recording by audioPath. The helper writes
   * contiguous PCM16LE 16 kHz mono from that capture's start (silence while paused) and closes the fds when it ends.
   */
  z.object({
    type: z.literal('speech.attach'),
    sessionId: z.string().min(1).max(256),
    target: z.discriminatedUnion('kind', [
      z.object({ kind: z.literal('dictation'), id: z.string() }),
      z.object({ kind: z.literal('recording'), audioPath: z.string() }),
    ]),
    channels: z
      .array(z.enum(['mic', 'system']))
      .min(1)
      .max(2),
  }),
  /**
   * ADR 0075: summarize with the on-device model. `prompt` is the whole user prompt, built and N69-fenced by the
   * host; the helper's instructions are fixed per `stage` (single = the whole meeting, map = one part, reduce =
   * merge part summaries). Answered by `summarize.result` or `summarize.error` with the same id.
   */
  z.object({
    type: z.literal('summarize'),
    id: SummarizeId,
    stage: z.enum(['single', 'map', 'reduce']),
    prompt: z.string().min(1).max(SL.promptMax),
  }),
  /** ADR 0075: stop a running `summarize` (the host's deadline passed); answered by `summarize.error` cancelled. */
  z.object({ type: z.literal('summarize.cancel'), id: SummarizeId }),
  /** ADR 0075: is the on-device model available? Answered by `summarize.status`. */
  z.object({ type: z.literal('summarize.check'), id: SummarizeId }),
  /** ADR 0217: nano's calendar access state, without asking. Answered by `calendar.access` with this id. */
  z.object({ type: z.literal('calendar.check'), id: CalendarReqId }),
  /**
   * ADR 0217: ask for full calendar access (the macOS prompt, once). Sent only after the user turned the calendar
   * on in nano or pressed 'connect calendar'; never from onboarding. Answered by `calendar.access`.
   */
  z.object({ type: z.literal('calendar.request'), id: CalendarReqId }),
  /** ADR 0217: read the events in [fromMs, toMs) (at most 15 days). Answered by `calendar.events`, or by
   * `calendar.access` when access isn't granted. */
  z.object({
    type: z.literal('calendar.read'),
    id: CalendarReqId,
    fromMs: z.number().int(),
    toMs: z.number().int(),
  }),
  /** Insert text into the focused app: AX selected text, then AX value, then pasteboard + Cmd-V. */
  z.object({
    type: z.literal('insert.text'),
    id: z.string(),
    text: z.string(),
    restoreClipboard: z.boolean(),
  }),
]);
export type HostToHelper = z.infer<typeof HostToHelper>;

export type ParsedLine =
  | { ok: true; msg: HelperToHost }
  | { ok: false; error: string; line: string };

/** Parse one stdout line from the helper. Never throws. */
export function parseHelperLine(line: string): ParsedLine {
  const trimmed = line.trim();
  if (!trimmed) return { ok: false, error: 'empty', line };
  let json: unknown;
  try {
    json = JSON.parse(trimmed);
  } catch {
    return { ok: false, error: 'invalid json', line };
  }
  const r = HelperToHost.safeParse(json);
  return r.success
    ? { ok: true, msg: r.data }
    : { ok: false, error: r.error.message, line };
}

export const encodeHostMessage = (m: HostToHelper): string =>
  JSON.stringify(HostToHelper.parse(m)) + '\n';

/** Splits a byte stream into lines, buffering partial lines across chunks. */
export function createLineSplitter(
  onLine: (line: string) => void,
  maxLine = 1_000_000,
) {
  let buf = '';
  return (chunk: string) => {
    buf += chunk;
    let i: number;
    while ((i = buf.indexOf('\n')) >= 0) {
      onLine(buf.slice(0, i));
      buf = buf.slice(i + 1);
    }
    if (buf.length > maxLine) buf = '';
  };
}

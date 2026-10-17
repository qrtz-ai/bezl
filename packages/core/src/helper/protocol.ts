import { z } from 'zod';
import { OsPermission } from '../capabilities';
import { AsrEvent, MeetingPlatform } from '../note/types';

/**
 * Wire protocol between the app (host) and the native Swift helper (`qrtz-helper`).
 * Transport: newline-delimited JSON over stdio. One message per line.
 * This file is the source of truth; the Swift side mirrors it (native/qrtz-helper/Sources/Protocol.swift).
 */
export const HELPER_PROTOCOL_VERSION = 1;

const PermissionState = z.enum([
  'granted',
  'denied',
  'not-determined',
  'unsupported',
]);

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
     */
    audioPath: z.string().optional(),
    activeAudioPath: z.string().optional(),
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
  z.object({ type: z.literal('asr'), event: AsrEvent }),
  z.object({ type: z.literal('recording.started'), audioPath: z.string() }),
  z.object({
    type: z.literal('recording.stopped'),
    audioPath: z.string(),
    durationMs: z.number(),
  }),
  // --- talk (dictation: mic only) ---
  /** Push-to-talk key from the helper's native key monitor (Electron shortcuts have no key-up). */
  z.object({ type: z.literal('hotkey'), action: z.enum(['down', 'up']) }),
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
  }),
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
  /** Privacy pause: the helper writes silence and transcribes nothing until resume. */
  z.object({ type: z.literal('recording.pause') }),
  z.object({ type: z.literal('recording.resume') }),
  /** Dictation settings: warm mic + pre-roll, cloud audio streaming, native push-to-talk key ('' = none). */
  z.object({
    type: z.literal('dictation.configure'),
    warmMic: z.boolean(),
    preRollMs: z.number().int().min(0).max(2000),
    streamAudio: z.boolean(),
    pushKey: z.string(),
  }),
  z.object({ type: z.literal('dictation.start'), id: z.string() }),
  z.object({ type: z.literal('dictation.stop'), id: z.string() }),
  z.object({ type: z.literal('dictation.cancel'), id: z.string() }),
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

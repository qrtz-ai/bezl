import { z } from 'zod';
import { OsPermission } from '../capabilities';

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

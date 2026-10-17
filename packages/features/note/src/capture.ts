import type { CaptureReport, SessionView } from '@qrtz/core';
import { DEGRADED_LABELS } from './useRecording';

/** Wall-clock time of day, 24 h "10:04" (the UI is lowercase, so no am/pm). */
export const fmtTimeOfDay = (ms: number) =>
  new Date(ms).toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  });

const SOURCE = { mic: 'mic', system: 'system audio' } as const;

/** Helper source names (`AudioSource.name`) as the user reads them. */
const CAPTURE_SOURCE_LABEL: Record<string, string> = {
  'core-audio-tap': 'meeting audio tap',
  screencapturekit: 'screen capture',
};
const sourceName = (n: string) => CAPTURE_SOURCE_LABEL[n] ?? n.toLowerCase();

/** N29 live badges: each silent source with its start as a time of day (`startedAt + sinceMs`). */
export function silentBadges(s: Pick<SessionView, 'silent' | 'startedAt'>) {
  const at = s.startedAt ?? Date.now();
  return (s.silent ?? []).map((x) => ({
    source: x.source,
    since: fmtTimeOfDay(at + x.sinceMs),
  }));
}

/**
 * N29/N32: everything the live `CaptureStatus` shows besides reconnect and error. The sticky degradations, then the
 * sources that are silent right now: one stronger no-audio badge when every delivered source is silent (`noAudio`),
 * else one per silent source with its start time. `audio-unavailable` already says there is no audio at all, so no
 * silence badge goes on top of it. Silence is live: a badge clears when the helper reports the source audible again.
 */
export function liveCaptureStatus(
  s: Pick<
    SessionView,
    'degraded' | 'silent' | 'noAudio' | 'startedAt' | 'asrUnavailable'
  >,
) {
  const degraded = s.degraded.map((f) => ({
    id: f,
    label:
      f === 'asr-unavailable' && s.asrUnavailable
        ? asrUnavailableLabel(s.asrUnavailable)
        : DEGRADED_LABELS[f],
  }));
  if (s.degraded.includes('audio-unavailable'))
    return { degraded, silent: [], noAudio: false };
  return {
    degraded,
    silent: s.noAudio ? [] : silentBadges(s),
    noAudio: !!s.noAudio,
  };
}

/**
 * N95 (ADR 0062): why live transcription stopped, as the live notice says it. Not installed → install it; an expired
 * or stale revocation list or a revoked release → connect to update; anything else → the audio is still recorded.
 */
const ASR_UNAVAILABLE_LABEL: Record<
  NonNullable<SessionView['asrUnavailable']>,
  string
> = {
  'not-installed': 'install qrtz speech',
  stale: 'connect to update',
  revoked: 'connect to update',
  crashed: 'speech stopped, audio is still recorded',
  'start-failed': 'speech stopped, audio is still recorded',
  timeout: 'speech stopped, audio is still recorded',
  'protocol-violations': 'speech stopped, audio is still recorded',
  network: 'speech stopped, audio is still recorded',
  cancelled: 'speech stopped, audio is still recorded',
};

/** Exhaustive over the zod enum: renaming a reason fails the typecheck instead of falling through to a default. */
export function asrUnavailableLabel(
  r: NonNullable<SessionView['asrUnavailable']>,
): string {
  return ASR_UNAVAILABLE_LABEL[r];
}

/** N35: what a failed capture device took away, as the meeting notice says it (not a permission revoke). */
const DEVICE_LOST_LABEL: Record<string, string> = {
  'no-mic': 'mic',
  'mic-only': 'system audio',
  'audio-unavailable': 'audio capture',
  'ax-unavailable': 'speaker names',
};

/** The sources a flag takes away (`SOURCES_LOST` in core lifecycle), to match a lost stretch to its device line. */
const LOST_BY: Record<string, readonly string[]> = {
  'no-mic': ['mic'],
  'mic-only': ['system'],
  'audio-unavailable': ['mic', 'system'],
};

/** Most lines the meeting notice shows; the rest collapse into "+n more". */
export const CAPTURE_NOTICE_MAX = 6;

/**
 * N29/H8 meeting notice: one short lowercase line per closed silent stretch and per source fallback, from the saved
 * pipeline capture record. Zero-length stretches (a source taken away, already shown as a degradation) are skipped;
 * identical fallbacks (a helper restarting) are counted once with ×n.
 */
export function captureNoticeLines(
  startedAt: number,
  capture: CaptureReport | undefined,
): { id: string; text: string; detail?: string }[] {
  const out: { id: string; text: string; detail?: string }[] = [];
  for (const [i, sp] of (capture?.silentSpans ?? []).entries()) {
    if (sp.endMs !== undefined && sp.endMs <= sp.startMs) continue;
    const from = fmtTimeOfDay(startedAt + sp.startMs);
    out.push({
      id: `silent-${i}`,
      text:
        sp.endMs === undefined
          ? `no audio from ${SOURCE[sp.source]} after ${from}`
          : `no audio from ${SOURCE[sp.source]} ${from}–${fmtTimeOfDay(startedAt + sp.endMs)}`,
    });
  }
  const seen = new Map<
    string,
    { id: string; text: string; detail?: string; n: number }
  >();
  for (const [i, f] of (capture?.fallbacks ?? []).entries()) {
    const text = f.to
      ? `${sourceName(f.from)} failed, used ${sourceName(f.to)}`
      : `${sourceName(f.from)} failed, no audio source left`;
    const prev = seen.get(text);
    if (prev) prev.n++;
    else seen.set(text, { id: `fallback-${i}`, text, detail: f.reason, n: 1 });
  }
  for (const f of seen.values())
    out.push({
      id: f.id,
      text: f.n > 1 ? `${f.text} ×${f.n}` : f.text,
      ...(f.detail ? { detail: f.detail } : {}),
    });
  // N35: a device that failed mid-recording (a revoke is not on this record, so this never reads as one).
  for (const [i, d] of (capture?.deviceFailures ?? []).entries())
    out.push({
      id: `device-${i}`,
      text: `${d.degraded.map((f) => DEVICE_LOST_LABEL[f] ?? f).join(', ')} device failed at ${fmtTimeOfDay(startedAt + d.atMs)}`,
      ...(d.reason ? { detail: d.reason } : {}),
    });
  // N39 / ADR 0036: a stretch in which a source was not recorded at all (a revoke, a start without the permission, no
  // source left). One that starts with a device failure or a "no source left" fallback at the same moment is already
  // said by that line, so it is skipped; one source per line, or one line when both were lost together.
  const explained = (src: 'mic' | 'system', atMs: number) =>
    (capture?.deviceFailures ?? []).some(
      (d) =>
        d.atMs === atMs && d.degraded.some((f) => LOST_BY[f]?.includes(src)),
    ) ||
    (capture?.fallbacks ?? []).some((f) => f.to === null && f.atMs === atMs);
  const lost = new Map<
    string,
    { sources: Set<string>; startMs: number; endMs?: number; i: number }
  >();
  for (const [i, sp] of (capture?.lostSpans ?? []).entries()) {
    if (sp.endMs !== undefined && sp.endMs <= sp.startMs) continue;
    if (explained(sp.source, sp.startMs)) continue;
    const key = `${sp.startMs}-${sp.endMs ?? ''}`;
    const g = lost.get(key);
    if (g) g.sources.add(sp.source);
    else
      lost.set(key, {
        sources: new Set([sp.source]),
        startMs: sp.startMs,
        endMs: sp.endMs,
        i,
      });
  }
  for (const g of lost.values()) {
    const [only] = [...g.sources] as ('mic' | 'system')[];
    const what =
      g.sources.size > 1 ? 'nothing recorded' : `${SOURCE[only!]} not recorded`;
    const from = fmtTimeOfDay(startedAt + g.startMs);
    out.push({
      id: `lost-${g.i}`,
      text:
        g.endMs === undefined
          ? `${what} after ${from}`
          : `${what} ${from}–${fmtTimeOfDay(startedAt + g.endMs)}`,
    });
  }
  if (out.length <= CAPTURE_NOTICE_MAX) return out;
  const shown = out.slice(0, CAPTURE_NOTICE_MAX - 1);
  return [...shown, { id: 'more', text: `+${out.length - shown.length} more` }];
}

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
  s: Pick<SessionView, 'degraded' | 'silent' | 'noAudio' | 'startedAt'>,
) {
  const degraded = s.degraded.map((f) => ({
    id: f,
    label: DEGRADED_LABELS[f],
  }));
  if (s.degraded.includes('audio-unavailable'))
    return { degraded, silent: [], noAudio: false };
  return {
    degraded,
    silent: s.noAudio ? [] : silentBadges(s),
    noAudio: !!s.noAudio,
  };
}

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
  if (out.length <= CAPTURE_NOTICE_MAX) return out;
  const shown = out.slice(0, CAPTURE_NOTICE_MAX - 1);
  return [...shown, { id: 'more', text: `+${out.length - shown.length} more` }];
}

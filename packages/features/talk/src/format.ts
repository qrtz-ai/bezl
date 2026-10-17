import type { TalkEntry } from '@qrtz/core';

const ENGINE: Record<TalkEntry['engine'], string> = {
  deepgram: 'deepgram',
  elevenlabs: 'elevenlabs',
  'on-device': 'on-device',
};

/** "10:42 · Notes · deepgram · 412 ms" (latency = key-up → text in the app). */
export function entryMeta(e: TalkEntry, locale?: string): string {
  const time = new Date(e.at).toLocaleTimeString(locale, {
    hour: 'numeric',
    minute: '2-digit',
  });
  const parts = [time];
  if (e.app) parts.push(e.app);
  parts.push(e.failover ? `${ENGINE[e.engine]} (fallback)` : ENGINE[e.engine]);
  const ms = e.latency.insertedMs ?? e.latency.finalMs;
  if (ms !== undefined) parts.push(`${Math.round(ms)} ms`);
  return parts.join(' · ');
}

/** "qrtz = cue arts, cue arts" → { term: 'qrtz', aliases: [...] }. */
export function parseTerm(
  input: string,
): { term: string; aliases: string[] } | null {
  const [term, rest] = input.split('=').map((s) => s?.trim() ?? '');
  if (!term) return null;
  return {
    term,
    aliases: (rest ?? '')
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean),
  };
}

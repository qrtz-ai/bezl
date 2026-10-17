import type { ActiveSpeakerEvent, Segment, SpeakerTurn } from './types';

/** AX placeholders that mean "someone, but we don't know who": they end the previous interval but never vote. */
const UNKNOWN =
  /^(|unknown|unknown speaker|participant|guest|someone|\.{2,}|…|-+|\?+)$/i;

/** Normalizes an AX name (whitespace, zero-width chars); returns undefined for unknown placeholders. */
export function normalizeSpeakerName(raw: string): string | undefined {
  const n = raw
    .replace(/[\u200b-\u200d\ufeff]/g, '')
    .replace(/\s+/g, ' ')
    .trim();
  return UNKNOWN.test(n) ? undefined : n;
}

/**
 * Turns the AX active-speaker signal (a step function: name holds until next event)
 * into intervals ending at `endMs`. Unknown/blank names end the previous interval and
 * produce no interval of their own; repeated names are merged.
 */
export function activeSpeakerIntervals(
  events: ActiveSpeakerEvent[],
  endMs: number,
) {
  const sorted = [...events].sort((a, b) => a.atMs - b.atMs);
  const out: { name: string; startMs: number; endMs: number }[] = [];
  sorted.forEach((e, i) => {
    const name = normalizeSpeakerName(e.name);
    const end = sorted[i + 1]?.atMs ?? endMs;
    if (!name || end <= e.atMs) return;
    const last = out.at(-1);
    if (last && last.name === name && last.endMs === e.atMs) last.endMs = end;
    else out.push({ name, startMs: e.atMs, endMs: end });
  });
  return out;
}

const overlap = (a0: number, a1: number, b0: number, b1: number) =>
  Math.max(0, Math.min(a1, b1) - Math.max(a0, b0));

/**
 * Maps diarization slots (Sortformer "S0".."S3") to real names using time-overlap-weighted
 * voting against the AX active-speaker intervals. Greedy one-to-one assignment by score,
 * so two slots never get the same name. Slots with weak evidence stay unnamed.
 */
export function mapSlotsToNames(
  turns: SpeakerTurn[],
  active: ActiveSpeakerEvent[],
  opts: {
    endMs: number;
    minShare?: number;
    /** AX highlight lags speech; turn windows are shifted by this much (default 300 ms). */
    lagMs?: number;
  } = { endMs: Infinity },
): Map<string, string> {
  const lag = opts.lagMs ?? 300;
  const intervals = activeSpeakerIntervals(active, opts.endMs + lag);
  const score = new Map<string, Map<string, number>>();
  const slotTotal = new Map<string, number>();
  for (const t of turns) {
    slotTotal.set(t.slot, (slotTotal.get(t.slot) ?? 0) + (t.endMs - t.startMs));
    // Time where another slot talks at once is ambiguous (AX highlights only one person), so it counts half.
    const others = turns.filter((u) => u.slot !== t.slot);
    for (const iv of intervals) {
      const a0 = Math.max(t.startMs + lag, iv.startMs);
      const a1 = Math.min(t.endMs + lag, iv.endMs);
      if (a1 <= a0) continue;
      let shared = 0;
      for (const u of others)
        shared += overlap(a0, a1, u.startMs + lag, u.endMs + lag);
      const o = a1 - a0 - Math.min(a1 - a0, shared) / 2;
      if (!o) continue;
      const m = score.get(t.slot) ?? new Map<string, number>();
      m.set(iv.name, (m.get(iv.name) ?? 0) + o);
      score.set(t.slot, m);
    }
  }
  const candidates: {
    slot: string;
    name: string;
    share: number;
    ms: number;
  }[] = [];
  for (const [slot, m] of score) {
    const total = slotTotal.get(slot) || 1;
    for (const [name, ms] of m)
      candidates.push({ slot, name, ms, share: ms / total });
  }
  candidates.sort((a, b) => b.ms - a.ms || b.share - a.share);
  const minShare = opts.minShare ?? 0.3;
  const result = new Map<string, string>();
  const usedNames = new Set<string>();
  for (const c of candidates) {
    if (result.has(c.slot) || usedNames.has(c.name) || c.share < minShare)
      continue;
    result.set(c.slot, c.name);
    usedNames.add(c.name);
  }
  return result;
}

/** Assigns each segment the slot it overlaps most, then the fused name. */
export function labelSegments(
  segments: Segment[],
  turns: SpeakerTurn[],
  names: Map<string, string>,
): Segment[] {
  return segments.map((s) => {
    let best: string | undefined;
    let bestMs = 0;
    for (const t of turns) {
      const o = overlap(s.startMs, s.endMs, t.startMs, t.endMs);
      if (o > bestMs) {
        bestMs = o;
        best = t.slot;
      }
    }
    if (!best) return s;
    const name = names.get(best);
    return { ...s, speaker: best, ...(name ? { speakerName: name } : {}) };
  });
}

/**
 * Live-only fusion: each segment takes the AX active speaker with the most overlap
 * (AX highlight lags speech slightly, so the window is shifted by `lagMs`).
 * Explicit names already on a segment are kept unless AX evidence exists.
 */
export function nameSegmentsFromAx(
  segments: Segment[],
  active: ActiveSpeakerEvent[],
  endMs: number,
  lagMs = 300,
  /** Minimum share of the segment the winning name must cover; below it the segment keeps its label. */
  minShare = 0.25,
): Segment[] {
  const ivs = activeSpeakerIntervals(
    active,
    Math.max(endMs, ...segments.map((s) => s.endMs + lagMs)),
  );
  return segments.map((s) => {
    const votes = new Map<string, number>();
    for (const iv of ivs) {
      const o = overlap(
        s.startMs + lagMs,
        s.endMs + lagMs,
        iv.startMs,
        iv.endMs,
      );
      if (o) votes.set(iv.name, (votes.get(iv.name) ?? 0) + o);
    }
    const [best, ms] =
      [...votes.entries()].sort((a, b) => b[1] - a[1])[0] ?? [];
    const dur = Math.max(1, s.endMs - s.startMs);
    return best && (ms ?? 0) / dur >= minShare
      ? { ...s, speakerName: best }
      : s;
  });
}

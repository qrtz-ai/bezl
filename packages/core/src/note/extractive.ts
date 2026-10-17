import type { Segment } from './types';
import type { MeetingSummary } from './summary';
import type { MeetingNote } from './notes';
import { templateById } from './templates';

const STOP = new Set(
  'a an the and or but if of to in on at for with is are was were be been it this that we you i they he she so just like yeah okay ok um uh do does did have has had not no yes our your my me us them from as by about can will would should could there here what when how who which'.split(
    ' ',
  ),
);

const ACTION_RE =
  /\b(i'll|i will|we'll|we will|you'll|will you|can you|could you|let's|need to|needs to|have to|must|action item|todo|to-do|follow up|follow-up|by (monday|tuesday|wednesday|thursday|friday|tomorrow|eod|end of (day|week)|next week))\b/i;

export function splitSentences(text: string): string[] {
  return text
    .split(/(?<=[.!?])\s+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/** N54: at most this many action items come from the transcript, then at most MAX_NOTE_ACTION_ITEMS from notes. */
export const MAX_ACTION_ITEMS = 50;
export const MAX_NOTE_ACTION_ITEMS = 20;
/** N54: at most this many Q&A pairs. */
export const MAX_QA = 30;

/**
 * For every position i: the first entry at or after i that passes `ok`, and the first one after that whose key
 * differs from the first one's (`-1`: none). `next(i, key)` is then the first entry at or after i that passes `ok`
 * and whose key differs from `key`, in O(1).
 */
function nextDifferent<T>(
  xs: T[],
  ok: (x: T) => boolean,
  key: (x: T) => string | undefined,
): (i: number, k: string | undefined) => number {
  const first = new Int32Array(xs.length + 1).fill(-1);
  const second = new Int32Array(xs.length + 1).fill(-1);
  for (let i = xs.length - 1; i >= 0; i--) {
    const f = first[i + 1]!;
    if (!ok(xs[i]!)) {
      first[i] = f;
      second[i] = second[i + 1]!;
    } else {
      first[i] = i;
      second[i] = f >= 0 && key(xs[f]!) === key(xs[i]!) ? second[i + 1]! : f;
    }
  }
  return (i, k) => {
    if (i >= xs.length) return -1;
    const f = first[i]!;
    return f < 0 || key(xs[f]!) !== k ? f : second[i]!;
  };
}

const words = (s: string) => s.toLowerCase().match(/[\p{L}\p{N}']+/gu) ?? [];

/**
 * Local, key-free summarizer. Deterministic so it can be golden-tested.
 * - summary: top-scoring sentences by content-word frequency (kept in transcript order)
 * - action items: commitment/request phrasing, owner from "I'll" → speaker, "can you" → next speaker / addressee
 * - Q&A: questions paired with the next utterance from a different speaker
 *
 * N54 (ADR 0048): the outputs are capped (MAX_ACTION_ITEMS from the transcript plus MAX_NOTE_ACTION_ITEMS from the
 * notes, MAX_QA) and the "next speaker" / "next answer" lookups are one backward pass, so a 3 h meeting is linear.
 * Up to the caps the output is the same as the old per-question scans.
 */
export function extractiveSummary(
  segments: Segment[],
  opts: {
    maxSentences?: number;
    notes?: MeetingNote[];
    template?: string;
  } = {},
): MeetingSummary {
  const tpl = templateById(opts.template);
  const focus = new Set(tpl.focus);
  type Sent = { text: string; speaker?: string; idx: number; seg: number };
  const sents: Sent[] = [];
  segments.forEach((s, seg) =>
    splitSentences(s.text).forEach((text) =>
      sents.push({
        text,
        seg,
        idx: sents.length,
        ...((s.speakerName ?? s.speaker)
          ? { speaker: s.speakerName ?? s.speaker }
          : {}),
      }),
    ),
  );
  const notes = (opts.notes ?? []).filter((n) => n.text.trim());
  const noteWords = new Set(
    notes
      .flatMap((n) => words(n.text))
      .filter((w) => !STOP.has(w) && w.length > 2),
  );
  const freq = new Map<string, number>();
  for (const s of sents)
    for (const w of words(s.text))
      if (!STOP.has(w) && w.length > 2) freq.set(w, (freq.get(w) ?? 0) + 1);
  const score = (s: Sent) => {
    const ws = words(s.text).filter((w) => !STOP.has(w) && w.length > 2);
    if (ws.length < 3) return 0;
    // Template focus words (e.g. "budget" for sales) and words the user wrote in their notes weigh more.
    const boost = ws.filter((w) => focus.has(w) || noteWords.has(w)).length;
    return (
      (ws.reduce((a, w) => a + (freq.get(w) ?? 0), 0) + boost * 3) /
      Math.sqrt(ws.length)
    );
  };
  const max = opts.maxSentences ?? 3;
  const top = [...sents]
    .filter((s) => !s.text.endsWith('?'))
    .sort((a, b) => score(b) - score(a) || a.idx - b.idx)
    .slice(0, max)
    .sort((a, b) => a.idx - b.idx);
  const transcriptPart = top
    .map((s) => (s.speaker ? `${s.speaker}: ${s.text}` : s.text))
    .join(' ');
  // The user's own notes lead the summary (they are what the user decided was important).
  const notesPart = notes.length
    ? `Your notes: ${notes.map((n) => n.text.trim().replace(/[.;]+$/, '')).join('; ')}.`
    : '';
  const summary = [notesPart, transcriptPart].filter(Boolean).join(' ');

  // The addressee of "Ana, can you …": the first speaker whose first name is Ana.
  const byFirstName = new Map<string, string>();
  for (const x of sents)
    if (x.speaker && !byFirstName.has(x.speaker.split(' ')[0]!))
      byFirstName.set(x.speaker.split(' ')[0]!, x.speaker);
  const nextSpeaker = nextDifferent(
    sents,
    (x) => !!x.speaker,
    (x) => x.speaker,
  );
  const actionItems: MeetingSummary['actionItems'] = [];
  for (const s of sents) {
    if (actionItems.length >= MAX_ACTION_ITEMS) break;
    if (!ACTION_RE.test(s.text)) continue;
    let owner = s.speaker;
    if (/\b(can you|could you|will you)\b/i.test(s.text)) {
      const named = /^(\p{Lu}\p{Ll}+),/u.exec(s.text)?.[1];
      const full = named ? (byFirstName.get(named) ?? named) : undefined;
      const next = nextSpeaker(s.idx + 1, s.speaker);
      owner = full ?? (next >= 0 ? sents[next]!.speaker : undefined);
    }
    actionItems.push({ text: s.text, ...(owner ? { owner } : {}) });
  }

  // Notes phrased as tasks ("TODO …", "- [ ] …", or commitment phrasing) become action items too.
  const fromTranscript = actionItems.length;
  for (const n of notes) {
    if (actionItems.length - fromTranscript >= MAX_NOTE_ACTION_ITEMS) break;
    const t = n.text.trim();
    const m = /^(?:todo:?|- \[ \]|\[ \]|action:?|ai:)\s*(.+)$/i.exec(t);
    if (m) actionItems.push({ text: m[1]!.trim() });
    else if (ACTION_RE.test(t)) actionItems.push({ text: t });
  }

  // The answer to a question: the first non-question of a later segment from another speaker (or none).
  const segStart = new Int32Array(segments.length + 1).fill(sents.length);
  for (let i = sents.length - 1; i >= 0; i--) segStart[sents[i]!.seg] = i;
  for (let g = segments.length - 1; g >= 0; g--)
    segStart[g] = Math.min(segStart[g]!, segStart[g + 1]!);
  const nextAnswer = nextDifferent(
    sents,
    (x) => !x.text.endsWith('?'),
    (x) => x.speaker,
  );
  const qa: MeetingSummary['qa'] = [];
  for (const s of sents) {
    if (qa.length >= MAX_QA) break;
    if (!s.text.endsWith('?')) continue;
    const a = nextAnswer(segStart[s.seg + 1]!, s.speaker);
    const ans = a >= 0 ? sents[a] : undefined;
    if (ans)
      qa.push({
        question: s.text,
        answer: ans.text,
        ...(s.speaker ? { askedBy: s.speaker } : {}),
      });
  }
  return {
    summary: summary || 'No speech was transcribed.',
    actionItems,
    qa,
    provider: 'extractive',
    template: tpl.id,
  };
}

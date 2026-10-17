import type {
  AsrEvent,
  ActiveSpeakerEvent,
  Segment,
  SpeakerTurn,
} from './types';

/** Scripted "standup" meeting used by the mock helper, unit tests and the e2e test. */
export const STANDUP_FIXTURE = {
  meeting: {
    key: 'zoom:fixture-standup',
    title: 'Weekly Product Sync',
    platform: 'zoom' as const,
    app: 'zoom.us',
  },
  participants: ['Ada Lovelace', 'Grace Hopper', 'Alan Turing'],
  /** [speaker, startMs, endMs, text] */
  lines: [
    [
      'Ada Lovelace',
      0,
      4200,
      'Welcome everyone, this is our weekly product sync for the qrtz desktop launch.',
    ],
    [
      'Grace Hopper',
      4400,
      9800,
      'The note recorder now captures system audio and the live transcript runs on the neural engine.',
    ],
    [
      'Ada Lovelace',
      10000,
      13500,
      'Great. What is the status of the summary pipeline?',
    ],
    [
      'Alan Turing',
      13800,
      19500,
      'The summary pipeline works offline with the extractive fallback and supports any LLM provider.',
    ],
    [
      'Ada Lovelace',
      19800,
      24000,
      'Grace, can you write the onboarding copy for accessibility permissions by Friday?',
    ],
    [
      'Grace Hopper',
      24300,
      27800,
      "Yes, I'll draft the onboarding copy tomorrow.",
    ],
    [
      'Alan Turing',
      28000,
      33000,
      'I will benchmark parakeet streaming latency on the M4 before the launch.',
    ],
    [
      'Ada Lovelace',
      33200,
      36000,
      "Perfect, let's ship the desktop launch next week. Thanks all.",
    ],
  ] as const,
};

export function fixtureAsrEvents(): AsrEvent[] {
  const out: AsrEvent[] = [];
  STANDUP_FIXTURE.lines.forEach(([, start, end, text], i) => {
    const words = text.split(' ');
    const id = `u${i}`;
    for (let k = 3; k < words.length; k += 4) {
      out.push({
        kind: 'partial',
        utteranceId: id,
        startMs: start,
        text: words.slice(0, k).join(' '),
      });
    }
    out.push({
      kind: 'final',
      utteranceId: id,
      startMs: start,
      endMs: end,
      text,
      confidence: 0.9,
    });
  });
  return out;
}

export function fixtureActiveSpeakers(lagMs = 250): ActiveSpeakerEvent[] {
  return STANDUP_FIXTURE.lines.map(([name, start]) => ({
    atMs: start + lagMs,
    name,
  }));
}

/** What the offline Parakeet + Sortformer pass would return (slots, not names). */
export function fixturePostResult(): {
  segments: Segment[];
  turns: SpeakerTurn[];
} {
  const slotOf = new Map<string, string>();
  const segments: Segment[] = [];
  const turns: SpeakerTurn[] = [];
  STANDUP_FIXTURE.lines.forEach(([name, start, end, text], i) => {
    if (!slotOf.has(name)) slotOf.set(name, `S${slotOf.size}`);
    const slot = slotOf.get(name)!;
    segments.push({
      id: `p${i}`,
      startMs: start,
      endMs: end,
      text,
      source: 'post',
      confidence: 0.97,
    });
    turns.push({ slot, startMs: start, endMs: end });
  });
  return { segments, turns };
}

export const FIXTURE_DURATION_MS = 36_500;

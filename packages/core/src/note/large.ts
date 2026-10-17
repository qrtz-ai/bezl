import type { Segment } from './types';

const SPEAKERS = [
  'Ada Lovelace',
  'Grace Hopper',
  'Alan Turing',
  'Katherine Johnson',
  'Linus Torvalds',
];
const PHRASES = [
  'Let us review the quarterly roadmap and the launch checklist.',
  'The latency numbers on the streaming pipeline look good this week.',
  'Can you take the action item to follow up with the design team by Friday?',
  'I think we should ship the onboarding flow behind a feature flag first.',
  'We still need a decision on pricing for the team plan.',
  'Search over transcripts is fast even for long meetings.',
  'I will write the release notes and send them out tomorrow.',
  'Does anyone have concerns about the migration plan?',
];

/** Deterministic synthetic meeting: one ~4.8 s segment per slot, rotating speakers (perf tests / dev seed). */
export function syntheticSegments(minutes: number): Segment[] {
  const out: Segment[] = [];
  const step = 4800;
  const n = Math.floor((minutes * 60_000) / step);
  for (let i = 0; i < n; i++) {
    out.push({
      id: `syn-${i}`,
      startMs: i * step,
      endMs: i * step + step - 300,
      text: `${PHRASES[(i * 7) % PHRASES.length]} (point ${i})`,
      speakerName: SPEAKERS[(i * 3 + (i >> 4)) % SPEAKERS.length]!,
      source: 'live',
    });
  }
  return out;
}

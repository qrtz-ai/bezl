/**
 * Shared Markdown export vectors (Tauri migration): spec/vectors/export/markdown.json. TS is the oracle;
 * crates/qrtz-backup/tests/markdown_vectors.rs must produce the same filename and bytes for every case.
 * `QRTZ_VECTORS_UPDATE=1 npx vitest run markdown.vectors` rewrites the file from the cases below.
 */
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { exportFilename, meetingToMarkdown } from './markdown';

const FILE = fileURLToPath(
  new URL('../../../../spec/vectors/export/markdown.json', import.meta.url),
);
const UPDATE = process.env.QRTZ_VECTORS_UPDATE === '1';

const base = {
  id: 'm1',
  title: 'Weekly sync',
  platform: 'zoom',
  startedAt: 1_760_000_000_000,
  status: 'done',
  participants: [] as string[],
};
const seg = (i: number, text: string, extra: Record<string, unknown> = {}) => ({
  id: `s${i}`,
  startMs: i * 61_500,
  endMs: i * 61_500 + 4000,
  text,
  source: 'live',
  ...extra,
});

/* eslint-disable @typescript-eslint/no-explicit-any */
const inputs: {
  name: string;
  meeting: any;
  segments: any[];
  summary?: any;
  notes?: any[];
}[] = [
  { name: 'minimal, no segments', meeting: base, segments: [] },
  {
    name: 'full: duration, participants, summary, actions with and without owner, Q&A, notes',
    meeting: {
      ...base,
      title: 'Acme pricing call',
      platform: 'google-meet',
      durationMs: 3_725_400,
      participants: ['Ada Lovelace', 'Bob'],
      endedAt: 1_760_003_725_400,
    },
    segments: [
      seg(0, 'Thanks for joining.', {
        speakerName: 'Ada Lovelace',
        speaker: 'S1',
      }),
      seg(1, 'Budget is 50k.', { speaker: 'S2' }),
      seg(70, 'Anything else?'),
    ],
    summary: {
      summary: 'We agreed on the pilot.\nNext step: security review.',
      actionItems: [
        { text: 'Send the questionnaire', owner: 'Bob' },
        { text: 'Book the follow-up' },
        { text: 'Empty owner', owner: '' },
      ],
      qa: [
        {
          question: 'When does it start?',
          answer: 'Next month.',
          askedBy: 'Ada Lovelace',
        },
        { question: 'Who pays?', answer: 'Acme.' },
      ],
      provider: 'openrouter:anthropic/claude-haiku-5.5',
      template: 'general',
    },
    notes: [
      { id: 'n1', atMs: 6200, text: 'Pilot budget 50k', createdAt: 1 },
      { id: 'n2', atMs: 3_661_000, text: 'TODO *security*', createdAt: 2 },
    ],
  },
  {
    name: 'summary with no action items and no Q&A',
    meeting: { ...base, platform: 'teams', durationMs: 59_999 },
    segments: [seg(1, 'hello')],
    summary: {
      summary: 'Short.',
      actionItems: [],
      qa: [],
      provider: 'extractive',
    },
  },
  {
    name: 'markdown metacharacters are escaped everywhere',
    meeting: {
      ...base,
      title: '# Q4 [draft] *plan* <v2> | a_b `x` \\ end',
      platform: 'slack-huddle',
      participants: ['_under_', 'pipe|name', '[x]'],
    },
    segments: [seg(2, '`code` **bold** <tag> #hash', { speakerName: '*Ann*' })],
    summary: {
      summary: '[link](x) _em_',
      actionItems: [{ text: 'a|b', owner: '<o>' }],
      qa: [{ question: '#?', answer: '`a`', askedBy: '_q_' }],
      provider: 'p',
    },
    notes: [{ id: 'n', atMs: 0, text: '<b>', createdAt: 0 }],
  },
  {
    name: 'unicode title: accents, CJK, emoji, compatibility forms',
    meeting: {
      ...base,
      title: 'Café résumé — 会议 🚀 ﬁnal Ｆｕｌｌ',
      platform: 'webex',
    },
    segments: [seg(3, '日本語のテキスト', { speakerName: '' })],
  },
  {
    name: 'long title is cut at 60 slug characters',
    meeting: {
      ...base,
      title:
        'An extremely long meeting title that keeps going well past the sixty character limit',
      platform: 'other',
    },
    segments: [],
  },
  {
    name: 'title with no letters or digits',
    meeting: { ...base, title: '!!! ??? ...', startedAt: 0 },
    segments: [],
  },
  {
    name: 'leap day, before 1970, negative segment offset clamps',
    meeting: {
      ...base,
      title: 'Leap',
      startedAt: 951_782_399_999,
      durationMs: 0,
    },
    segments: [seg(0, 'early', { startMs: -500 })],
  },
  {
    name: 'year before 1970',
    meeting: { ...base, title: 'Old', startedAt: -86_400_001 },
    segments: [],
  },
];
/* eslint-enable @typescript-eslint/no-explicit-any */

const compute = () => ({
  $comment:
    'Markdown export vectors. TS (packages/core/src/note/markdown.vectors.test.ts) is the oracle; crates/qrtz-backup/tests/markdown_vectors.rs must match filename and markdown byte for byte.',
  cases: inputs.map((c) => ({
    ...c,
    notes: c.notes ?? [],
    expected: {
      filename: exportFilename(c.meeting),
      markdown: meetingToMarkdown(
        c.meeting,
        c.segments,
        c.summary,
        c.notes ?? [],
      ),
    },
  })),
});

describe('markdown export vectors', () => {
  it('match spec/vectors/export/markdown.json', () => {
    const now = compute();
    if (UPDATE || !existsSync(FILE))
      writeFileSync(FILE, `${JSON.stringify(now, null, 1)}\n`);
    expect(JSON.parse(readFileSync(FILE, 'utf8'))).toEqual(
      JSON.parse(JSON.stringify(now)),
    );
  });
});

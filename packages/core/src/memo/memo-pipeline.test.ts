import { describe, expect, it } from 'vitest';
import { DatabaseSync } from 'node:sqlite';
import { migrate, migrations } from '../db/migrate';
import { MemoRepo, toFtsQuery } from './repo';
import { extractiveSummary } from './extractive';
import {
  createProvider,
  parseSummaryReply,
  summarizeMeeting,
  type FetchLike,
} from './llm';
import { exportFilename, meetingToMarkdown } from './markdown';
import { SessionMachine, sessionReducer } from './session';
import {
  STANDUP_FIXTURE,
  fixtureActiveSpeakers,
  fixturePostResult,
} from './fixtures';
import { labelSegments, mapSlotsToNames, nameSegmentsFromAx } from './fusion';
import { createMockHelper } from '../helper/mock';
import {
  encodeHostMessage,
  parseHelperLine,
  type HelperToHost,
} from '../helper/protocol';
import { FakeClock } from '../testing';

const fused = () => {
  const { segments, turns } = fixturePostResult();
  const names = mapSlotsToNames(turns, fixtureActiveSpeakers(), {
    endMs: 40_000,
  });
  return labelSegments(segments, turns, names);
};
const meeting = {
  id: 'm1',
  title: STANDUP_FIXTURE.meeting.title,
  platform: 'zoom' as const,
  startedAt: Date.UTC(2026, 9, 16, 22, 0),
  endedAt: Date.UTC(2026, 9, 16, 22, 1),
  status: 'done' as const,
  participants: [...STANDUP_FIXTURE.participants],
  durationMs: 36_500,
};

describe('live AX fusion', () => {
  it('names live fixture segments from the lagging AX highlight', () => {
    const live = fixturePostResult().segments.map((x) => ({
      ...x,
      source: 'live' as const,
    }));
    expect(
      nameSegmentsFromAx(live, fixtureActiveSpeakers(), 40_000).map(
        (x) => x.speakerName,
      ),
    ).toEqual(STANDUP_FIXTURE.lines.map((l) => l[0]));
  });
});

describe('fusion on fixture', () => {
  it('names every post segment from AX active speaker', () => {
    expect(fused().map((s) => s.speakerName)).toEqual(
      STANDUP_FIXTURE.lines.map((l) => l[0]),
    );
  });
});

describe('extractive summary', () => {
  const s = extractiveSummary(fused());
  it('finds action items with owners', () => {
    expect(s.provider).toBe('extractive');
    const items = s.actionItems.map((a) => [a.owner, a.text]);
    expect(items).toContainEqual([
      'Grace Hopper',
      expect.stringMatching(/onboarding copy for accessibility/),
    ]);
    expect(items).toContainEqual([
      'Grace Hopper',
      "Yes, I'll draft the onboarding copy tomorrow.",
    ]);
    expect(items).toContainEqual([
      'Alan Turing',
      expect.stringMatching(/benchmark parakeet/),
    ]);
  });
  it('pairs questions with answers', () => {
    expect(s.qa[0]).toMatchObject({
      askedBy: 'Ada Lovelace',
      answer: expect.stringMatching(/extractive fallback/),
    });
  });
  it('produces a non-empty summary', () => {
    expect(s.summary.length).toBeGreaterThan(40);
  });
});

describe('LLM layer', () => {
  const reply = {
    summary: 'S',
    actionItems: [{ text: 'Do X', owner: 'Ada' }],
    qa: [],
  };
  const fakeFetch =
    (body: unknown, ok = true): FetchLike =>
    async () => ({
      ok,
      status: ok ? 200 : 500,
      json: async () => body,
      text: async () => 'err',
    });
  it('openai-compatible provider parses JSON reply', async () => {
    const calls: { url: string; headers: Record<string, string> }[] = [];
    const f: FetchLike = async (url, init) => {
      calls.push({ url, headers: init.headers });
      return {
        ok: true,
        status: 200,
        json: async () => ({
          choices: [
            {
              message: {
                content: '```json\n' + JSON.stringify(reply) + '\n```',
              },
            },
          ],
        }),
        text: async () => '',
      };
    };
    const p = createProvider('openai', { fetch: f, apiKey: 'sk-test' })!;
    const s = await summarizeMeeting('t', fused(), p);
    expect(s).toEqual({ ...reply, provider: 'openai' });
    expect(calls[0]!.url).toBe('https://api.openai.com/v1/chat/completions');
    expect(calls[0]!.headers.authorization).toBe('Bearer sk-test');
  });
  it('anthropic + google adapters', async () => {
    const a = createProvider('anthropic', {
      fetch: fakeFetch({
        content: [{ type: 'text', text: JSON.stringify(reply) }],
      }),
      apiKey: 'k',
    })!;
    expect((await summarizeMeeting('t', fused(), a)).provider).toBe(
      'anthropic',
    );
    const g = createProvider('google', {
      fetch: fakeFetch({
        candidates: [{ content: { parts: [{ text: JSON.stringify(reply) }] } }],
      }),
      apiKey: 'k',
    })!;
    expect((await summarizeMeeting('t', fused(), g)).provider).toBe('google');
  });
  it('falls back to extractive without key, on HTTP error, or on bad JSON', async () => {
    expect(createProvider('openai', { fetch: fakeFetch({}) })).toBeNull();
    const errs: unknown[] = [];
    const bad = createProvider('openai', {
      fetch: fakeFetch({}, false),
      apiKey: 'k',
    });
    expect(
      (await summarizeMeeting('t', fused(), bad, (e) => errs.push(e))).provider,
    ).toBe('extractive');
    expect(errs).toHaveLength(1);
    expect(() => parseSummaryReply('no json here')).toThrow();
  });
});

describe('MemoRepo + FTS (DB integration)', () => {
  const setup = () => {
    const db = new DatabaseSync(':memory:');
    db.exec('PRAGMA foreign_keys = ON');
    migrate(db, migrations);
    const repo = new MemoRepo(db);
    repo.upsertMeeting(meeting);
    repo.replaceSegments('m1', 'post', fused());
    repo.saveSummary('m1', extractiveSummary(fused()));
    repo.upsertMeeting({
      ...meeting,
      id: 'm2',
      title: 'Hiring loop',
      startedAt: meeting.startedAt + 1,
    });
    repo.appendSegment('m2', {
      id: 'l0',
      startMs: 0,
      endMs: 1000,
      text: 'Candidate debrief for the designer role',
      source: 'live',
    });
    return repo;
  };
  it('lists newest first and reads segments + summary', () => {
    const repo = setup();
    expect(repo.listMeetings().map((m) => m.id)).toEqual(['m2', 'm1']);
    expect(repo.getSegments('m1')).toHaveLength(8);
    expect(repo.getSegments('m2')[0]!.source).toBe('live');
    expect(repo.getSummary('m1')?.provider).toBe('extractive');
    expect(repo.getMeeting('m1')?.participants).toEqual(
      STANDUP_FIXTURE.participants,
    );
  });
  it('searches transcripts (prefix + stemming), titles and summaries', () => {
    const repo = setup();
    expect(repo.search('neural eng').map((h) => h.meetingId)).toEqual(['m1']);
    expect(repo.search('benchmarking')[0]).toMatchObject({
      meetingId: 'm1',
      snippet: expect.stringContaining('['),
    });
    expect(repo.search('designer').map((h) => h.meetingId)).toEqual(['m2']);
    expect(repo.search('hiring').map((h) => h.meetingId)).toEqual(['m2']);
    expect(repo.search('"); DROP TABLE meetings; --')).toEqual([]);
    expect(repo.listMeetings()).toHaveLength(2);
  });
  it('re-processing replaces segments and keeps FTS consistent; delete cascades', () => {
    const repo = setup();
    repo.replaceSegments('m1', 'post', [
      {
        id: 'x',
        startMs: 0,
        endMs: 1,
        text: 'totally different words',
        source: 'post',
      },
    ]);
    expect(repo.search('neural')).toEqual([]);
    expect(repo.search('different')[0]?.meetingId).toBe('m1');
    repo.deleteMeeting('m1');
    expect(repo.search('different')).toEqual([]);
    expect(repo.getMeeting('m1')).toBeUndefined();
  });
  it('sanitizes FTS queries', () => {
    expect(toFtsQuery('  "foo" OR bar* ')).toBe('"foo"* "or"* "bar"*');
    expect(toFtsQuery('***')).toBeNull();
  });
});

describe('markdown export', () => {
  it('matches golden output', () => {
    const md = meetingToMarkdown(meeting, fused(), extractiveSummary(fused()));
    expect(md).toMatchSnapshot();
    expect(md).toContain('# Weekly Product Sync');
    expect(md).toContain('- [ ] ');
    expect(md).toContain('**[0:04] Grace Hopper:**');
    expect(exportFilename(meeting)).toBe('2026-10-16-weekly-product-sync.md');
  });
});

describe('session state machine', () => {
  const d = { key: 'k', title: 't', platform: 'zoom' as const, app: 'zoom.us' };
  it('walks the happy path and rejects illegal transitions', () => {
    const m = new SessionMachine();
    expect(m.send({ type: 'stop', at: 1 })).toBe(false);
    expect(m.send({ type: 'detected', meeting: d })).toBe(true);
    expect(m.send({ type: 'start', meetingId: 'm', at: 5 })).toBe(true);
    expect(m.send({ type: 'detected', meeting: d })).toBe(false);
    expect(m.send({ type: 'reset' })).toBe(false);
    expect(m.send({ type: 'stop', at: 9 })).toBe(true);
    expect(m.state).toMatchObject({
      phase: 'processing',
      startedAt: 5,
      stoppedAt: 9,
      meetingId: 'm',
    });
    expect(m.send({ type: 'processed' })).toBe(true);
    expect(m.send({ type: 'reset' })).toBe(true);
  });
  it('drops a detected meeting when it ends before recording', () => {
    const s = sessionReducer(
      { phase: 'detected', detected: d },
      { type: 'meeting-ended', key: 'k' },
    );
    expect(s).toEqual({ phase: 'idle' });
  });
});

describe('mock helper scripted meeting (protocol integration)', () => {
  it('detects, streams ASR + speakers, and stops', () => {
    const clock = new FakeClock();
    const h = createMockHelper({ clock, scenario: 'standup', speed: 0.01 });
    const got: HelperToHost[] = [];
    h.onLine((l) => {
      const r = parseHelperLine(l);
      if (r.ok) got.push(r.msg);
    });
    clock.advance(400);
    expect(got.map((m) => m.type)).toContain('meeting.detected');
    h.write(
      encodeHostMessage({ type: 'recording.start', audioPath: '/tmp/a.wav' }),
    );
    clock.advance(400);
    const finals = got.filter(
      (m) => m.type === 'asr' && m.event.kind === 'final',
    );
    expect(finals).toHaveLength(8);
    expect(got.some((m) => m.type === 'ax.activeSpeaker')).toBe(true);
    h.write(encodeHostMessage({ type: 'recording.stop' }));
    expect(got.at(-1)).toMatchObject({
      type: 'recording.stopped',
      durationMs: 36_500,
    });
  });
});

import { describe, expect, it } from 'vitest';
import {
  STANDUP_FIXTURE,
  fixtureActiveSpeakers,
  fixtureAsrEvents,
  TranscriptAssembler,
  type FetchLike,
} from '@qrtz/core';
import { openDatabase } from './db';
import { createWorkerHandlers } from './handlers';

const noFetch: FetchLike = async () => {
  throw new Error('network disabled in tests');
};

function seed(h: ReturnType<typeof createWorkerHandlers>) {
  h['memo.create']({
    meeting: {
      id: 'm1',
      title: STANDUP_FIXTURE.meeting.title,
      platform: 'zoom',
      startedAt: 1_760_000_000_000,
      status: 'recording',
      participants: [...STANDUP_FIXTURE.participants],
    },
  });
  const a = new TranscriptAssembler();
  for (const e of fixtureAsrEvents()) {
    a.push(e);
    if (e.kind === 'final')
      h['memo.appendLive']({
        meetingId: 'm1',
        segment: a.segments().find((s) => s.id === e.utteranceId)!,
      });
  }
}

describe('worker handlers (DB + pipeline integration)', () => {
  it('records live, finalizes the live transcript with AX speaker fusion + extractive summary, lists, searches and exports', async () => {
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: noFetch,
    });
    expect(h['db.health']()).toEqual({
      ok: true,
      schemaVersion: 2,
      fts5: true,
    });
    seed(h);
    expect(h['memo.get']({ id: 'm1' }).segments).toHaveLength(8);
    const summary = await h['memo.finalize']({
      meetingId: 'm1',
      endedAt: 1_760_000_036_500,
      durationMs: 36_500,
      audioPath: '/tmp/m1.wav',
      active: fixtureActiveSpeakers(),
      llm: { provider: 'none' },
    });
    expect(summary.provider).toBe('extractive');
    expect(summary.actionItems.length).toBeGreaterThanOrEqual(3);
    const got = h['memo.get']({ id: 'm1' });
    expect(got.meeting).toMatchObject({
      status: 'done',
      durationMs: 36_500,
      audioPath: '/tmp/m1.wav',
    });
    expect(
      got.segments.every((s) => s.source === 'live' && s.speakerName),
    ).toBe(true);
    expect(h['memo.list']({}).map((r) => r.meeting.id)).toEqual(['m1']);
    expect(
      h['memo.list']({ query: 'onboarding copy' })[0]?.hit?.snippet,
    ).toMatch(/\[onboarding\]/);
    expect(h['memo.list']({ query: 'zebra' })).toEqual([]);
    const ex = h['memo.export']({ id: 'm1' });
    expect(ex.filename).toMatch(/weekly-product-sync\.md$/);
    expect(ex.markdown).toContain('## Action items');
  });

  it('names live segments from AX and falls back to extractive when the LLM fails', async () => {
    const logs: string[] = [];
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: noFetch,
      log: (m) => logs.push(m),
    });
    seed(h);
    const s = await h['memo.finalize']({
      meetingId: 'm1',
      endedAt: 1,
      durationMs: 36_500,
      active: fixtureActiveSpeakers(),
      llm: { provider: 'openai', apiKey: 'sk-test' },
    });
    expect(s.provider).toBe('extractive');
    expect(logs).toContain('llm failed; using extractive fallback');
    expect(h['memo.get']({ id: 'm1' }).segments[1]?.speakerName).toBe(
      'Grace Hopper',
    );
  });

  it('rejects unknown meetings with E_NOT_FOUND', () => {
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: noFetch,
    });
    expect(() => h['memo.get']({ id: 'nope' })).toThrow(/not found/);
  });
});

import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import {
  STANDUP_FIXTURE,
  fixtureActiveSpeakers,
  fixtureAsrEvents,
  TranscriptAssembler,
  type FetchLike,
} from '@qrtz/core';
import { openDatabase } from './db';
import { createWorkerHandlers } from './handlers';

/**
 * End-to-end over real HTTP: worker handlers -> createProvider('openrouter') -> global fetch ->
 * a local OpenAI-compatible stub. Proves the OpenRouter path (auth, headers, JSON mode, reply
 * parsing, fallback + retry) without a real key.
 */
type Req = { path: string; headers: http.IncomingHttpHeaders; body: any };
const seen: Req[] = [];
let mode: 'ok' | 'fail' | 'garbage' | 'chat' = 'ok';
let server: http.Server;
let baseUrl = '';

beforeAll(async () => {
  server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (c) => (raw += c));
    req.on('end', () => {
      seen.push({
        path: req.url ?? '',
        headers: req.headers,
        body: JSON.parse(raw || '{}'),
      });
      if (mode === 'fail') {
        res.writeHead(429, { 'content-type': 'application/json' });
        return res.end(JSON.stringify({ error: { message: 'rate limited' } }));
      }
      const content =
        mode === 'chat'
          ? 'Grace will draft the onboarding copy [1].'
          : mode === 'garbage'
            ? 'sorry, I cannot help'
            : '```json\n' +
              JSON.stringify({
                summary:
                  'The team reviewed the launch and agreed on next steps.',
                actionItems: [{ text: 'Ship the beta', owner: 'Ada Lovelace' }],
                qa: [{ question: 'When is launch?', answer: 'Friday' }],
              }) +
              '\n```';
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          id: 'x',
          choices: [{ index: 0, message: { role: 'assistant', content } }],
        }),
      );
    });
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  baseUrl = `http://127.0.0.1:${(server.address() as AddressInfo).port}/api/v1`;
});
afterAll(() => new Promise<void>((r) => server.close(() => r())));

function setup() {
  const h = createWorkerHandlers(openDatabase(':memory:'), {
    fetch: fetch as unknown as FetchLike,
  });
  h['note.create']({
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
      h['note.appendLive']({
        meetingId: 'm1',
        segment: a.segments().find((s) => s.id === e.utteranceId)!,
      });
  }
  return h;
}

const llm = () => ({
  provider: 'openrouter' as const,
  apiKey: 'sk-test-stub',
  baseUrl,
});
const finalize = (h: ReturnType<typeof setup>) =>
  h['note.finalize']({
    meetingId: 'm1',
    endedAt: 1_760_000_036_500,
    durationMs: 36_500,
    active: fixtureActiveSpeakers(),
    llm: llm(),
  });

describe('OpenRouter summary path (local OpenAI-compatible stub)', () => {
  it('sends an authenticated chat completion and stores the parsed LLM summary', async () => {
    mode = 'ok';
    seen.length = 0;
    const h = setup();
    const s = await finalize(h);
    expect(s!.provider).toBe('openrouter');
    expect(s!.llmError).toBeUndefined();
    expect(s!.actionItems).toEqual([
      { text: 'Ship the beta', owner: 'Ada Lovelace' },
    ]);
    expect(h['note.get']({ id: 'm1' }).summary?.provider).toBe('openrouter');
    const r = seen[0]!;
    expect(r.path).toBe('/api/v1/chat/completions');
    expect(r.headers.authorization).toBe('Bearer sk-test-stub');
    expect(r.headers['x-title']).toBe('qrtz');
    expect(r.body.model).toBeTruthy();
    expect(JSON.stringify(r.body.messages)).toContain('Transcript:');
  });

  it('falls back to extractive with llmError on HTTP errors, then retry succeeds', async () => {
    mode = 'fail';
    const h = setup();
    const s = await finalize(h);
    expect(s!.provider).toBe('extractive');
    expect(s!.llmError).toMatch(/429|rate/i);
    expect(h['note.get']({ id: 'm1' }).meeting.status).toBe('done');
    mode = 'ok';
    const r = await h['note.resummarize']({ id: 'm1', llm: llm() });
    expect(r.provider).toBe('openrouter');
    expect(h['note.get']({ id: 'm1' }).summary?.llmError).toBeUndefined();
  });

  it('treats a non-JSON reply as a failure (fallback, not a crash)', async () => {
    mode = 'garbage';
    const s = await finalize(setup());
    expect(s!.provider).toBe('extractive');
    expect(s!.llmError).toMatch(/JSON/);
  });

  it('chat: per-meeting and across-meetings answers are grounded in retrieved lines', async () => {
    mode = 'chat';
    seen.length = 0;
    const h = setup();
    await finalize(h);
    seen.length = 0;
    const a = await h['note.chat']({
      question: 'who writes the onboarding copy?',
      meetingId: 'm1',
      llm: llm(),
    });
    expect(a.provider).toBe('openrouter');
    expect(a.answer).toContain('[1]');
    expect(a.sources[0]!.text.toLowerCase()).toContain('onboarding');
    const prompt = JSON.stringify(seen[0]!.body.messages);
    expect(prompt).toContain('Excerpts:');
    expect(prompt.toLowerCase()).toContain('onboarding');
    const all = await h['note.chat']({
      question: 'onboarding copy',
      llm: llm(),
    });
    expect(all.sources.length).toBeGreaterThan(0);
    expect(all.sources.every((s) => s.meetingId === 'm1')).toBe(true);
  });

  it('chat: LLM failure falls back to extractive quotes; no key = extractive', async () => {
    mode = 'fail';
    const h = setup();
    const a = await h['note.chat']({
      question: 'onboarding copy',
      meetingId: 'm1',
      llm: llm(),
    });
    expect(a.provider).toBe('extractive');
    expect(a.llmError).toMatch(/429/);
    expect(a.answer).toMatch(/onboarding/i);
    const b = await h['note.chat']({
      question: 'zzzqqq nothing',
      llm: { provider: 'none' },
    });
    expect(b.sources).toEqual([]);
    expect(b.answer).toMatch(/couldn't find/);
  });

  it('speaker rename and merge update lines and participants', async () => {
    mode = 'ok';
    const h = setup();
    await finalize(h);
    const before = h['note.get']({ id: 'm1' });
    const names = [
      ...new Set(
        before.segments.map((s) => s.speakerName ?? s.speaker).filter(Boolean),
      ),
    ] as string[];
    expect(names.length).toBeGreaterThan(1);
    const r = h['note.renameSpeaker']({
      meetingId: 'm1',
      from: names[0]!,
      to: 'Renamed Person',
    });
    expect(r.changed).toBeGreaterThan(0);
    h['note.renameSpeaker']({
      meetingId: 'm1',
      from: names[1]!,
      to: 'Renamed Person',
    }); // merge
    const after = h['note.get']({ id: 'm1' });
    const left = new Set(after.segments.map((s) => s.speakerName ?? s.speaker));
    expect(left.has(names[0]!)).toBe(false);
    expect(left.has(names[1]!)).toBe(false);
    expect(left.has('Renamed Person')).toBe(true);
    expect(after.meeting.participants).toContain('Renamed Person');
    expect(after.meeting.participants).not.toContain(names[0]);
  });

  it('search results are paged (limit + offset)', async () => {
    const h = setup();
    const p1 = h['note.list']({ query: 'onboarding', limit: 1 });
    expect(p1.length).toBeLessThanOrEqual(2);
    expect(
      h['note.list']({ query: 'onboarding', limit: 1, offset: 1 }),
    ).toHaveLength(0);
  });
});

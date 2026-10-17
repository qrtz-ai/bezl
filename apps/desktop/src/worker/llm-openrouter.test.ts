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
let mode: 'ok' | 'fail' | 'garbage' = 'ok';
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
        mode === 'garbage'
          ? 'sorry, I cannot help'
          : '```json\n' +
            JSON.stringify({
              summary: 'The team reviewed the launch and agreed on next steps.',
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
  return h;
}

const llm = () => ({
  provider: 'openrouter' as const,
  apiKey: 'sk-test-stub',
  baseUrl,
});
const finalize = (h: ReturnType<typeof setup>) =>
  h['memo.finalize']({
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
    expect(s.provider).toBe('openrouter');
    expect(s.llmError).toBeUndefined();
    expect(s.actionItems).toEqual([
      { text: 'Ship the beta', owner: 'Ada Lovelace' },
    ]);
    expect(h['memo.get']({ id: 'm1' }).summary?.provider).toBe('openrouter');
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
    expect(s.provider).toBe('extractive');
    expect(s.llmError).toMatch(/429|rate/i);
    expect(h['memo.get']({ id: 'm1' }).meeting.status).toBe('done');
    mode = 'ok';
    const r = await h['memo.resummarize']({ id: 'm1', llm: llm() });
    expect(r.provider).toBe('openrouter');
    expect(h['memo.get']({ id: 'm1' }).summary?.llmError).toBeUndefined();
  });

  it('treats a non-JSON reply as a failure (fallback, not a crash)', async () => {
    mode = 'garbage';
    const s = await finalize(setup());
    expect(s.provider).toBe('extractive');
    expect(s.llmError).toMatch(/JSON/);
  });
});

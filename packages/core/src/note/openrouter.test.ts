import fs from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  OPENROUTER_DEFAULT_MODEL,
  PROVIDER_DEFAULTS,
  createProvider,
  needsCredits,
  summarizeMeeting,
  type FetchLike,
} from './llm';
import type { Segment } from './types';

const reply = (status: number, body: unknown) =>
  ({
    ok: status < 400,
    status,
    text: async () => JSON.stringify(body),
    json: async () => body,
  }) as unknown as Response;

/** Fake OpenRouter: 402 unless the account has credits. Records requested models. */
function fakeOpenRouter(paidOk = false) {
  const asked: string[] = [];
  const f: FetchLike = async (_url, init) => {
    const { model } = JSON.parse(String(init?.body)) as { model: string };
    asked.push(model);
    if (!paidOk)
      return reply(402, { error: { message: 'Insufficient credits' } });
    return reply(200, {
      choices: [{ message: { content: `ok from ${model}` } }],
    });
  };
  return { f, asked };
}

describe('OpenRouter default model (Claude Haiku 5.5)', () => {
  it('defaults to anthropic/claude-haiku-5.5', async () => {
    expect(OPENROUTER_DEFAULT_MODEL).toBe('anthropic/claude-haiku-5.5');
    expect(PROVIDER_DEFAULTS.openrouter.model).toBe(OPENROUTER_DEFAULT_MODEL);
    const { f, asked } = fakeOpenRouter(true);
    const p = createProvider('openrouter', { fetch: f, apiKey: 'k' })!;
    expect(await p.complete([{ role: 'user', content: 'hi' }])).toBe(
      'ok from anthropic/claude-haiku-5.5',
    );
    expect(asked).toEqual(['anthropic/claude-haiku-5.5']);
  });

  it('on 402 surfaces a clear credits error (no free-model fallback)', async () => {
    const { f, asked } = fakeOpenRouter();
    const p = createProvider('openrouter', { fetch: f, apiKey: 'k' })!;
    const e = await p
      .complete([{ role: 'user', content: 'hi' }])
      .catch((x: Error) => x);
    expect(String(e)).toMatch(/402.*no OpenRouter credits.*retry/);
    expect(needsCredits(e)).toBe(true);
    expect(asked).toEqual(['anthropic/claude-haiku-5.5']);
  });

  it('summaries keep the on-device fallback on 402, with the error attached', async () => {
    const { f } = fakeOpenRouter();
    const p = createProvider('openrouter', { fetch: f, apiKey: 'k' })!;
    const s = await summarizeMeeting(
      'Sync',
      [
        {
          id: 's0',
          meetingId: 'm',
          startMs: 0,
          endMs: 4000,
          speaker: 'A',
          text: 'We will ship the beta on Friday.',
          source: 'live',
        } as Segment,
      ],
      p,
    );
    expect(s.provider).toBe('extractive');
    expect(s.llmError).toMatch(/credits/);
  });

  it('does not treat a bad key as a credits problem', () => {
    expect(
      needsCredits(new Error('LLM HTTP 401: No auth credentials found')),
    ).toBe(false);
    expect(needsCredits(new Error('LLM HTTP 402: Payment required'))).toBe(
      true,
    );
    expect(needsCredits(new Error('LLM HTTP 403: Insufficient credits'))).toBe(
      true,
    );
  });
});

/**
 * Opt-in live check against the real OpenRouter API (one summary call). Off by default and in CI.
 *   QRTZ_TEST_OPENROUTER=1 npx vitest run packages/core/src/note/openrouter.test.ts
 * The key is read from QRTZ_OPENROUTER_KEY_FILE (default /home/box/.secrets/openrouter_qrtz.key) and
 * never printed.
 */
const live = process.env.QRTZ_TEST_OPENROUTER === '1';
const keyFile =
  process.env.QRTZ_OPENROUTER_KEY_FILE ??
  '/home/box/.secrets/openrouter_qrtz.key';
describe.skipIf(!live)('OpenRouter live (QRTZ_TEST_OPENROUTER=1)', () => {
  it('makes one real summary call', { timeout: 90_000 }, async () => {
    expect(fs.existsSync(keyFile), `missing key file ${keyFile}`).toBe(true);
    const apiKey = fs.readFileSync(keyFile, 'utf8').trim();
    const p = createProvider('openrouter', { fetch, apiKey })!;
    const seg = (i: number, speaker: string, text: string): Segment =>
      ({
        id: `s${i}`,
        meetingId: 'm',
        startMs: i * 5000,
        endMs: i * 5000 + 4000,
        speaker,
        speakerName: speaker,
        text,
        source: 'live',
      }) as Segment;
    let err: unknown = null;
    const s = await summarizeMeeting(
      'Launch sync',
      [
        seg(0, 'Grace', "Let's ship the beta on Friday."),
        seg(1, 'Jared', 'Agreed. I will send the recap to the team today.'),
        seg(2, 'Grace', 'Pricing moves to next week.'),
      ],
      p,
      (e) => (err = e),
    );
    expect(err, String(err)).toBeNull();
    expect(s.provider).not.toBe('extractive');
    expect(s.summary.length).toBeGreaterThan(20);
    console.log(
      `live OpenRouter summary ok via ${s.provider}: ${s.summary.slice(0, 120)}`,
    );
  });
});

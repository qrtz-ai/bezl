/**
 * Hardening cycle 3 stress scenarios (keep running on every `npm test`). See docs/hardening/LOG.md.
 * LLM transport after a 200 (N8), OpenRouter error bodies, malformed model replies.
 */
import { describe, expect, it } from 'vitest';
import { AppError } from '../errors';
import { createProvider, summarizeMeeting, type FetchLike } from './llm';

const segs = [
  {
    id: 's0',
    startMs: 0,
    endMs: 900,
    text: 'We ship on Friday.',
    source: 'live' as const,
  },
];
const orWith = (fetch: FetchLike) =>
  createProvider('openrouter', { fetch, apiKey: 'sk-test' })!;
/** A 200 whose body read fails with `e` (what undici does when the socket resets mid-body). */
const okThenBody =
  (body: () => Promise<unknown>): FetchLike =>
  async () => ({
    ok: true,
    status: 200,
    json: body,
    text: async () => '',
  });
const strict = (fetch: FetchLike, signal?: AbortSignal) =>
  summarizeMeeting('T', segs, orWith(fetch), undefined, {
    strict: true,
    ...(signal ? { signal } : {}),
  }).then(
    () => {
      throw new Error('expected the strict summary to throw');
    },
    (e: unknown) => e as AppError,
  );

describe('hardening 3: LLM failures after the status line (N8)', () => {
  it('a connection reset while reading a 200 body is a retryable E_UNAVAILABLE, not E_INTERNAL', async () => {
    const err = await strict(
      okThenBody(() => Promise.reject(new TypeError('terminated'))),
    );
    expect(err).toBeInstanceOf(AppError);
    expect(err.code).toBe('E_UNAVAILABLE');
    expect(err.retryable).toBe(true);
  });

  it('a 200 with an HTML body (proxy, captive portal) is a retryable provider error', async () => {
    const err = await strict(
      okThenBody(() =>
        Promise.reject(new SyntaxError('Unexpected token < in JSON')),
      ),
    );
    expect(err).toBeInstanceOf(AppError);
    expect(err.code).toBe('E_PROVIDER');
    expect(err.retryable).toBe(true);
  });

  it('the deadline firing during the body read is E_TIMEOUT', async () => {
    const ac = new AbortController();
    const err = await strict(
      okThenBody(() => {
        ac.abort();
        return Promise.reject(
          Object.assign(new Error('aborted'), { name: 'AbortError' }),
        );
      }),
      ac.signal,
    );
    expect(err.code).toBe('E_TIMEOUT');
    expect(err.retryable).toBe(true);
  });

  it('OpenRouter upstream errors inside a 200 surface their message; 5xx retryable, 4xx terminal', async () => {
    const body = (error: unknown) => okThenBody(async () => ({ error }));
    const e502 = await strict(
      body({ code: 502, message: 'upstream overloaded' }),
    );
    expect(e502.code).toBe('E_PROVIDER');
    expect(e502.message).toContain('upstream overloaded');
    expect(e502.retryable).toBe(true);
    const e400 = await strict(body({ code: 400, message: 'context too long' }));
    expect(e400.retryable).toBe(false);
  });

  it('a malformed model reply (prose, wrong shape) is retryable so the pipeline re-asks before falling back', async () => {
    for (const content of [
      'Sorry, I cannot help with that.',
      JSON.stringify({ summary: 5, actionItems: 'none' }),
    ]) {
      const err = await strict(
        okThenBody(async () => ({ choices: [{ message: { content } }] })),
      );
      expect(err).toBeInstanceOf(AppError);
      expect(err.code).toBe('E_PROVIDER');
      expect(err.retryable).toBe(true);
    }
  });

  it('non-strict callers still get the extractive fallback with the LLM error attached', async () => {
    const s = await summarizeMeeting(
      'T',
      segs,
      orWith(okThenBody(() => Promise.reject(new TypeError('terminated')))),
    );
    expect(s.provider).toBe('extractive');
    expect(s.llmError).toContain('terminated');
  });
});

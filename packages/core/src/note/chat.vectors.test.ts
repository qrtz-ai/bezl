/**
 * Shared chat vectors (spec/vectors/llm/chat.json, N89, ADR 0059): `buildChatPrompt` (N69/N85 fence, history
 * window) with the exact OpenRouter request body, and `answerQuestion` over pre-ranked sources (the shape of the
 * ChatAnswer: LLM answer trimmed, the 24-source cap, the 3-line extractive fallback with `llmError`). The Rust port
 * (crates/qrtz-llm/tests/n89.rs) runs the same file against a mock server and must produce the same bytes.
 *
 * TS is the oracle: `QRTZ_VECTORS_UPDATE=1 bunx vitest run chat.vectors` rewrites every `expect`.
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import {
  answerQuestion,
  buildChatPrompt,
  type ChatSource,
  type ChatTurn,
} from './chat';
import { createProvider, type FetchLike } from './llm';

const FILE = fileURLToPath(
  new URL('../../../../spec/vectors/llm/chat.json', import.meta.url),
);
const UPDATE = process.env.QRTZ_VECTORS_UPDATE === '1';

type Reply = { status: number; body: string };
type V = {
  prompts: {
    name: string;
    question: string;
    sources: ChatSource[];
    history?: ChatTurn[];
    nonce: string;
    expect?: unknown;
  }[];
  answers: {
    name: string;
    question: string;
    sources?: ChatSource[];
    gen?: number;
    history?: ChatTurn[];
    reply: Reply | null;
    expect?: unknown;
  }[];
};

/** `gen: n`: n generated sources (see the `about`). */
const genSources = (n: number): ChatSource[] =>
  Array.from({ length: n }, (_, i) => ({
    meetingId: `m${i % 3}`,
    title: `Meeting ${i % 3}`,
    startedAt: 1_700_000_000_000 + (i % 3),
    startMs: i * 7_000,
    ...(i % 4 === 3 ? {} : { speaker: `S${i % 2}` }),
    text: `line ${i} about the budget`,
  }));

const recorder = (reply: Reply) => {
  const seen: { body: string }[] = [];
  const fetch: FetchLike = async (_url, init) => {
    seen.push({ body: init.body });
    return new Response(reply.body, {
      status: reply.status,
      headers: { 'content-type': 'application/json' },
    });
  };
  return { seen, fetch };
};

describe('shared chat vectors (spec/vectors/llm/chat.json)', () => {
  it('buildChatPrompt + request body, answerQuestion shaping', async () => {
    const v = JSON.parse(readFileSync(FILE, 'utf8')) as V;
    for (const c of v.prompts) {
      const msgs = buildChatPrompt(
        c.question,
        c.sources,
        c.history ?? [],
        c.nonce,
      );
      const rec = recorder({
        status: 200,
        body: JSON.stringify({ choices: [{ message: { content: 'ok' } }] }),
      });
      await createProvider('openrouter', {
        fetch: rec.fetch,
        apiKey: 'sk-or-test',
      })!.complete(msgs);
      const got = { messages: msgs, body: rec.seen[0]!.body };
      if (UPDATE) c.expect = got;
      else expect(got, c.name).toEqual(c.expect);
    }
    for (const c of v.answers) {
      const rec = c.reply ? recorder(c.reply) : null;
      const provider = rec
        ? createProvider('openrouter', {
            fetch: rec.fetch,
            apiKey: 'sk-or-test',
          })
        : null;
      const got = await answerQuestion(
        c.question,
        c.sources ?? genSources(c.gen!),
        provider,
        c.history ?? [],
        true,
      );
      if (UPDATE) c.expect = got;
      else expect(got, c.name).toEqual(c.expect);
    }
    if (UPDATE) writeFileSync(FILE, `${JSON.stringify(v, null, 1)}\n`);
  });
});

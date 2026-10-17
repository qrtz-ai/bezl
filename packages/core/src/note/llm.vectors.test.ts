/**
 * Shared LLM vectors (spec/vectors/llm/*.json): the summary prompt (N21 strided excerpts and surrogate-safe cuts,
 * N29/N34/N39 capture gaps, notes, templates), the request body sent to OpenRouter, the provider reply
 * classification (N8: errors in a 200, HTML, bad shape), the reply parser and the on-device extractive fallback.
 * The Rust port (crates/qrtz-llm/tests/vectors.rs) runs the same files and must produce the same bytes.
 *
 * TS is the oracle: `QRTZ_VECTORS_UPDATE=1 npx vitest run llm.vectors` rewrites every `expect`.
 */
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { AppError } from '../errors';
import { extractiveSummary } from './extractive';
import {
  OPENROUTER_DEFAULT_MODEL,
  buildSummaryPrompt,
  captureGapsForPrompt,
  createProvider,
  formatTranscriptForPrompt,
  parseSummaryReply,
  type FetchLike,
} from './llm';
import type { Segment } from './types';

const DIR = fileURLToPath(
  new URL('../../../../spec/vectors/llm/', import.meta.url),
);
const UPDATE = process.env.QRTZ_VECTORS_UPDATE === '1';

type Gen = {
  count: number;
  stepMs: number;
  speakers: (string | null)[];
  texts: string[];
  repeat?: number;
};
/** Generated transcripts (kept out of the JSON): see the `about` of prompts.json. */
const genSegments = (g: Gen): Segment[] =>
  Array.from({ length: g.count }, (_, i) => {
    const sp = g.speakers[i % g.speakers.length];
    return {
      id: `g${i}`,
      startMs: i * g.stepMs,
      endMs: i * g.stepMs + g.stepMs - 100,
      text: `${g.texts[i % g.texts.length]!.repeat(g.repeat ?? 1)} #${i}`,
      source: 'live' as const,
      ...(sp != null ? { speakerName: sp } : {}),
    };
  });
const segsOf = (c: { segments?: Segment[]; gen?: Gen }) =>
  c.segments ?? genSegments(c.gen!);

/** Long strings are pinned by length (UTF-16 units) + sha256 of the UTF-8 bytes, plus head and tail. */
const pin = (s: string): unknown =>
  s.length <= 4000
    ? s
    : {
        length: s.length,
        sha256: createHash('sha256').update(s, 'utf8').digest('hex'),
        head: Array.from(s).slice(0, 100).join(''),
        tail: Array.from(s).slice(-100).join(''),
      };

const errOf = (e: unknown) => {
  const a = AppError.from(e);
  return [
    a.code,
    a.retryable,
    a.message.replace(
      /^(LLM reply was not JSON|LLM reply did not match the summary shape): [\s\S]*$/,
      '$1: …',
    ),
  ];
};

const file = (name: string) => `${DIR}${name}.json`;
const load = (name: string) =>
  JSON.parse(readFileSync(file(name), 'utf8')) as Record<string, unknown>;
const save = (name: string, v: unknown) =>
  writeFileSync(file(name), `${JSON.stringify(v, null, 1)}\n`);

/** Every request the provider makes, recorded instead of sent; replies with `reply`. */
const recorder = (reply: { status: number; body: string; type?: string }) => {
  const seen: { url: string; headers: Record<string, string>; body: string }[] =
    [];
  const fetch: FetchLike = async (url, init) => {
    seen.push({ url, headers: init.headers, body: init.body });
    return new Response(reply.body, {
      status: reply.status,
      headers: { 'content-type': reply.type ?? 'application/json' },
    });
  };
  return { seen, fetch };
};

describe('shared LLM vectors (spec/vectors/llm)', () => {
  it('prompts.json: buildSummaryPrompt, the OpenRouter request, formatTranscriptForPrompt, captureGapsForPrompt', async () => {
    const v = load('prompts') as {
      prompts: {
        title: string;
        segments?: Segment[];
        gen?: Gen;
        opts: Record<string, unknown>;
        expect?: unknown;
      }[];
      transcripts: {
        segments?: Segment[];
        gen?: Gen;
        maxChars: number;
        expect?: unknown;
      }[];
      gaps: {
        spans: never[];
        durationMs?: number;
        lost?: never[];
        expect?: unknown;
      }[];
    };
    for (const c of v.prompts) {
      const msgs = buildSummaryPrompt(c.title, segsOf(c), c.opts);
      const rec = recorder({
        status: 200,
        body: JSON.stringify({ choices: [{ message: { content: '{}' } }] }),
      });
      await createProvider('openrouter', {
        fetch: rec.fetch,
        apiKey: 'sk-or-test',
      })!.complete(msgs, { json: true });
      const req = rec.seen[0]!;
      const got = {
        messages: msgs.map((m) => ({ role: m.role, content: pin(m.content) })),
        request: { url: req.url, headers: req.headers, body: pin(req.body) },
      };
      if (UPDATE) c.expect = got;
      else expect(got).toEqual(c.expect);
    }
    for (const c of v.transcripts) {
      const got = pin(formatTranscriptForPrompt(segsOf(c), c.maxChars));
      if (UPDATE) c.expect = got;
      else expect(got).toEqual(c.expect);
    }
    for (const c of v.gaps) {
      const got = captureGapsForPrompt(c.spans, c.durationMs, c.lost ?? []);
      if (UPDATE) c.expect = got;
      else expect(got).toEqual(c.expect);
    }
    if (UPDATE) save('prompts', v);
    expect(OPENROUTER_DEFAULT_MODEL).toBe('anthropic/claude-haiku-5.5');
  });

  it('replies.json: provider reply classification (N8)', async () => {
    const v = load('replies') as {
      cases: {
        status: number;
        body: string;
        contentType?: string;
        expect?: unknown;
      }[];
    };
    for (const c of v.cases) {
      const rec = recorder({
        status: c.status,
        body: c.body,
        ...(c.contentType ? { type: c.contentType } : {}),
      });
      const p = createProvider('openrouter', {
        fetch: rec.fetch,
        apiKey: 'sk-or-test',
      })!;
      let got: unknown;
      try {
        got = { ok: await p.complete([{ role: 'user', content: 'hi' }]) };
      } catch (e) {
        got = { err: errOf(e) };
      }
      if (UPDATE) c.expect = got;
      else expect(got).toEqual(c.expect);
    }
    if (UPDATE) save('replies', v);
  });

  it('summary-replies.json: parseSummaryReply', () => {
    const v = load('summary-replies') as {
      cases: { text: string; expect?: unknown }[];
    };
    for (const c of v.cases) {
      let got: unknown;
      try {
        got = { ok: JSON.parse(JSON.stringify(parseSummaryReply(c.text))) };
      } catch (e) {
        got = { err: errOf(e) };
      }
      if (UPDATE) c.expect = got;
      else expect(got).toEqual(c.expect);
    }
    if (UPDATE) save('summary-replies', v);
  });

  it('extractive.json: the on-device fallback summary', () => {
    const v = load('extractive') as {
      cases: {
        segments: Segment[];
        opts: Parameters<typeof extractiveSummary>[1];
        expect?: unknown;
      }[];
    };
    for (const c of v.cases) {
      const got = JSON.parse(
        JSON.stringify(extractiveSummary(c.segments, c.opts)),
      );
      if (UPDATE) c.expect = got;
      else expect(got).toEqual(c.expect);
    }
    if (UPDATE) save('extractive', v);
  });
});

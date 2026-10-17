import { describe, expect, it } from 'vitest';
import {
  STANDUP_FIXTURE,
  fixtureActiveSpeakers,
  fixtureAsrEvents,
  TranscriptAssembler,
  type Embedder,
  type FetchLike,
} from '@qrtz/core';
import { openDatabase } from './db';
import { createWorkerHandlers } from './handlers';
import { EMBED_MODEL, transformersEmbedder } from './embedder';

const noFetch: FetchLike = async () => {
  throw new Error('offline');
};

/** Deterministic fake: maps words to concept axes so paraphrases share vectors (no lexical overlap needed). */
const CONCEPTS: Record<string, number> = {
  onboarding: 0,
  newcomers: 0,
  new: 0,
  users: 0,
  welcome: 0,
  copy: 1,
  text: 1,
  docs: 1,
  wording: 1,
  draft: 1,
  write: 1,
  latency: 2,
  speed: 2,
  fast: 2,
  slow: 2,
  benchmark: 2,
};
const fake: Embedder = {
  id: 'fake-concepts',
  async embed(texts) {
    return texts.map((t) => {
      const v = new Float32Array(8);
      for (const w of t.toLowerCase().match(/[a-z]+/g) ?? [])
        if (w in CONCEPTS) v[CONCEPTS[w]!] += 1;
      v[7] = 0.05;
      const n = Math.hypot(...v);
      return v.map((x) => x / n);
    });
  },
};

async function setup(embedder?: () => Promise<Embedder | null>) {
  const h = createWorkerHandlers(openDatabase(':memory:'), {
    fetch: noFetch,
    ...(embedder ? { embedder } : {}),
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
  await h['note.finalize']({
    meetingId: 'm1',
    endedAt: 1_760_000_036_500,
    durationMs: 36_500,
    active: fixtureActiveSpeakers(),
    llm: { provider: 'none' },
  });
  return h;
}

const PARAPHRASE = 'who prepares the wording for newcomers?'; // no shared terms with "onboarding copy"

describe('semantic chat retrieval (hybrid)', () => {
  it('finds paraphrased lines that full-text search misses', async () => {
    const lexical = await setup();
    const a = await lexical['note.chat']({
      question: PARAPHRASE,
      llm: { provider: 'none' },
    });
    expect(a.sources).toEqual([]); // FTS alone: nothing

    const h = await setup(async () => fake);
    const idx = await h['note.indexVectors']({});
    expect(idx.indexed).toBeGreaterThan(0);
    const b = await h['note.chat']({
      question: PARAPHRASE,
      llm: { provider: 'none' },
    });
    expect(b.sources[0]?.text.toLowerCase()).toContain('onboarding');
    const c = await h['note.chat']({
      question: PARAPHRASE,
      meetingId: 'm1',
      llm: { provider: 'none' },
    });
    expect(c.sources[0]?.text.toLowerCase()).toContain('onboarding');
  });

  it('falls back to full-text when the embedder fails to load', async () => {
    const h = await setup(async () => {
      throw new Error('model download failed');
    });
    const a = await h['note.chat']({
      question: 'onboarding copy',
      meetingId: 'm1',
      llm: { provider: 'none' },
    });
    expect(a.sources[0]?.text.toLowerCase()).toContain('onboarding');
    expect((await h['note.indexVectors']({})).indexed).toBe(0);
  });

  // Real MiniLM via transformers.js; opt in (downloads ~23 MB): QRTZ_TEST_EMBEDDINGS=1 npm test
  it.skipIf(process.env.QRTZ_TEST_EMBEDDINGS !== '1')(
    `real ${EMBED_MODEL} retrieves a paraphrase`,
    async () => {
      const h = await setup(transformersEmbedder('/tmp/qrtz-test-models'));
      const b = await h['note.chat']({
        question: 'Who is preparing the text for new users?',
        llm: { provider: 'none' },
      });
      expect(
        b.sources
          .slice(0, 3)
          .map((s) => s.text.toLowerCase())
          .join(' '),
      ).toContain('onboarding');
    },
    120_000,
  );
});

/**
 * Shared ranker vectors (spec/vectors/search/rank.json): TS rankSources / queryTerms / hybridRank are the oracle for
 * crates/qrtz-search/tests/rank.rs. `QRTZ_VECTORS_UPDATE=1` rewrites the expectations.
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { queryTerms, rankSources, type ChatSource } from './chat';
import { hybridRank, type VecSource } from './embeddings';

const FILE = fileURLToPath(
  new URL('../../../../spec/vectors/search/rank.json', import.meta.url),
);
const UPDATE = process.env.QRTZ_VECTORS_UPDATE === '1';

interface Spec {
  about: string;
  sources: (ChatSource & { vec?: number[] })[];
  cases: {
    question: string;
    k: number;
    qvec?: number[];
    terms?: string[];
    lexical?: string[];
    hybrid?: string[];
  }[];
}

describe('shared ranker vectors (spec/vectors/search/rank.json)', () => {
  it('queryTerms, rankSources and hybridRank', () => {
    const v = JSON.parse(readFileSync(FILE, 'utf8')) as Spec;
    const plain: ChatSource[] = v.sources.map(({ vec: _v, ...s }) => s);
    const withVec: VecSource[] = v.sources.map(({ vec, ...s }) =>
      vec ? { ...s, vec: Float32Array.from(vec) } : s,
    );
    const id = (s: ChatSource) => `${s.meetingId}@${s.startMs}`;
    for (const c of v.cases) {
      const got = {
        terms: queryTerms(c.question),
        lexical: rankSources(c.question, plain, c.k).map(id),
        hybrid: hybridRank(
          c.question,
          withVec,
          c.qvec ? Float32Array.from(c.qvec) : null,
          c.k,
        ).map(id),
      };
      if (UPDATE) Object.assign(c, got);
      else
        expect(got, c.question).toEqual({
          terms: c.terms,
          lexical: c.lexical,
          hybrid: c.hybrid,
        });
    }
    if (UPDATE) writeFileSync(FILE, `${JSON.stringify(v, null, 1)}\n`);
  });
});

/**
 * Shared talk cleanup vectors (spec/vectors/talk/cleanup.json). The Rust talk service runs the same file
 * (crates/qrtz-talk/tests/vectors.rs), so both cleanups agree byte for byte. TS is the oracle:
 * `QRTZ_VECTORS_UPDATE=1 npx vitest run cleanup.vectors` recomputes every `expect`.
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { applyDictionary, plausibleCleanup, ruleCleanup } from './cleanup';
import type { DictionaryEntry } from './contract';

const FILE = fileURLToPath(
  new URL('../../../../spec/vectors/talk/cleanup.json', import.meta.url),
);
const UPDATE = process.env.QRTZ_VECTORS_UPDATE === '1';

interface Vectors {
  dictionary: DictionaryEntry[];
  rules: { raw: string; dict: boolean; expect: string | null }[];
  dictionaryOnly: { text: string; expect: string | null }[];
  plausible: { raw: string; out: string; expect: boolean | null }[];
}

describe('talk cleanup vectors', () => {
  const v = JSON.parse(readFileSync(FILE, 'utf8')) as Vectors;
  it('match the TS cleanup', () => {
    for (const r of v.rules) {
      const got = ruleCleanup(r.raw, r.dict ? v.dictionary : []);
      if (UPDATE) r.expect = got;
      else expect(got, r.raw).toBe(r.expect);
    }
    for (const d of v.dictionaryOnly) {
      const got = applyDictionary(d.text, v.dictionary);
      if (UPDATE) d.expect = got;
      else expect(got, d.text).toBe(d.expect);
    }
    for (const p of v.plausible) {
      const got = plausibleCleanup(p.raw, p.out);
      if (UPDATE) p.expect = got;
      else expect(got, p.out).toBe(p.expect);
    }
    if (UPDATE) writeFileSync(FILE, `${JSON.stringify(v, null, 2)}\n`);
  });
});

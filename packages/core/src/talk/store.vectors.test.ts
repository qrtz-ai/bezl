/**
 * Shared talk.json vectors (spec/vectors/talk/store.json): the Electron TalkService load/save rules, run by the Rust
 * talk store too (crates/qrtz-talk/tests/vectors.rs), so a history written by either build reads the same in the
 * other. TS is the oracle: `QRTZ_VECTORS_UPDATE=1 npx vitest run store.vectors`.
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { DictionaryEntry, TalkEntry } from './contract';

const FILE = fileURLToPath(
  new URL('../../../../spec/vectors/talk/store.json', import.meta.url),
);
const UPDATE = process.env.QRTZ_VECTORS_UPDATE === '1';

describe('talk store vectors', () => {
  it('load keeps valid entries and saves the same bytes', () => {
    const v = JSON.parse(readFileSync(FILE, 'utf8')) as {
      file: { history: unknown[]; dictionary: unknown[] };
      expect?: {
        history: string[];
        dictionary: string[];
        dropped: number;
        serialized: string;
      };
    };
    // Mirrors TalkService.load (apps/desktop/src/main/talk-service.ts) entry by entry.
    const keep = <T>(
      xs: unknown[],
      s: { safeParse: (x: unknown) => { success: boolean; data?: T } },
    ) =>
      xs.flatMap((x) => {
        const r = s.safeParse(x);
        return r.success ? [r.data as T] : [];
      });
    const history = keep<TalkEntry>(v.file.history, TalkEntry);
    const dictionary = keep<DictionaryEntry>(
      v.file.dictionary,
      DictionaryEntry,
    );
    const got = {
      history: history.map((h) => h.id),
      dictionary: dictionary.map((d) => d.term),
      dropped:
        v.file.history.length +
        v.file.dictionary.length -
        history.length -
        dictionary.length,
      serialized: JSON.stringify({ history, dictionary }),
    };
    if (UPDATE) {
      v.expect = got;
      writeFileSync(FILE, `${JSON.stringify(v, null, 2)}\n`);
    } else expect(got).toEqual(v.expect);
  });
});

/**
 * Shared cloud/on-device arbitration vectors (spec/vectors/talk/engine-arbiter.json). The Rust talk service runs
 * the same file (crates/qrtz-talk/tests/vectors.rs). TS is the oracle:
 * `QRTZ_VECTORS_UPDATE=1 npx vitest run engine-arbiter.vectors` recomputes every `expect`.
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { EngineArbiter, type CloudStream } from './asr';
import type { TalkEngineId } from './contract';

const FILE = fileURLToPath(
  new URL('../../../../spec/vectors/talk/engine-arbiter.json', import.meta.url),
);
const UPDATE = process.env.QRTZ_VECTORS_UPDATE === '1';

type Step =
  | { op: 'attach'; engine: Exclude<TalkEngineId, 'on-device'>; at: number }
  | { op: 'cloud'; committed: string; partial: string; at: number }
  | { op: 'cloudError'; error: string; at: number }
  | { op: 'localPartial' | 'localFinal'; text: string; at: number }
  | { op: 'check'; at: number };
interface Snapshot {
  usingCloud: boolean;
  engine: TalkEngineId;
  view: [string, string];
}
interface Case {
  name: string;
  stallMs: number | null;
  steps: (Step & { expect?: Snapshot })[];
  finish: { local: string; cloud: { ok: string } | { err: string } | null };
  expect?: {
    text: string;
    engine: TalkEngineId;
    failover: boolean;
    reason: string | null;
  };
}

describe('engine arbiter vectors', () => {
  const v = JSON.parse(readFileSync(FILE, 'utf8')) as { cases: Case[] };
  it('has the expected coverage', () => expect(v.cases.length).toBe(12));
  for (const c of v.cases)
    it(c.name, async () => {
      let now = 0;
      const fin = c.finish.cloud;
      let a: EngineArbiter;
      const cloud = (
        engine: Exclude<TalkEngineId, 'on-device'>,
      ): CloudStream => ({
        engine,
        push: () => undefined,
        finish: () =>
          fin === null
            ? Promise.reject(new Error('unused'))
            : 'ok' in fin
              ? Promise.resolve(fin.ok)
              : new Promise<string>(() => a.cloudError(fin.err)),
        close: () => undefined,
      });
      a = new EngineArbiter({
        cloud: null,
        stallMs: c.stallMs ?? Infinity,
        finalMs: 5,
        now: () => now,
      });
      for (const s of c.steps) {
        now = s.at;
        if (s.op === 'attach') a.attachCloud(cloud(s.engine));
        else if (s.op === 'cloud') a.cloudUpdate(s.committed, s.partial);
        else if (s.op === 'cloudError') a.cloudError(s.error);
        else if (s.op === 'localPartial') a.localPartial(s.text);
        else if (s.op === 'localFinal') a.localFinal(s.text);
        else a.check();
        const view = a.view();
        const snap: Snapshot = {
          usingCloud: a.usingCloud,
          engine: a.engine,
          view: [view.committed, view.partial],
        };
        if (UPDATE) s.expect = snap;
        else expect(snap, `${c.name} @${s.at} ${s.op}`).toEqual(s.expect);
      }
      const r = await a.finish(c.finish.local);
      const got = {
        text: r.text,
        engine: r.engine,
        failover: r.failover,
        reason: r.reason ?? null,
      };
      if (UPDATE) c.expect = got;
      else expect(got).toEqual(c.expect);
    });
  if (UPDATE)
    it('writes the oracle', () =>
      writeFileSync(FILE, `${JSON.stringify(v, null, 2)}\n`));
});

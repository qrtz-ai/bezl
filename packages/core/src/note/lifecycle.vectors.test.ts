/**
 * Shared tier-1 lifecycle vectors (spec/vectors/lifecycle/*.json). The same files are run by the Rust core
 * (crates/qrtz-lifecycle/tests/vectors.rs), so both implementations must agree on every transition, byte for byte
 * (exact JSON equality, not partial matches).
 *
 * TS is the oracle during the dual run: `QRTZ_VECTORS_UPDATE=1 bunx vitest run lifecycle.vectors` recomputes every
 * `expect` from this implementation and rewrites the files. Add a scenario by writing its inputs with
 * `"expect": null` and running the update.
 */
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import {
  RETRY_POLICIES,
  RecordingState,
  initialRecordingState,
  lifecycleReducer,
  newPipeline,
  nextRunnableStep,
  pipelineSettled,
  pipelineStep,
  pipelineUsable,
  retryDelay,
  type LifecycleEvent,
  type PipelineContext,
  type PipelineState,
  type RetryPolicy,
  type StepId,
  type StopReason,
} from './lifecycle';

const DIR = fileURLToPath(
  new URL('../../../../spec/vectors/lifecycle/', import.meta.url),
);
const UPDATE = process.env.QRTZ_VECTORS_UPDATE === '1';

/** JSON round trip: drops undefined exactly like the persisted/IPC form. */
const plain = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

type Rejected = 'rejected';
interface SessionVector {
  kind: 'session';
  name: string;
  initial: unknown;
  steps: { event: LifecycleEvent; expect: RecordingState | Rejected | null }[];
}
interface SessionMatrix {
  kind: 'session-matrix';
  name: string;
  states: Record<string, unknown>;
  events: LifecycleEvent[];
  expect: Record<string, (RecordingState | Rejected)[]> | null;
}
type Patch = Parameters<typeof pipelineStep>[2];
interface PipelineVector {
  kind: 'pipeline';
  name: string;
  initial: {
    meetingId: string;
    stopReason: StopReason;
    ctx: PipelineContext;
    now: number;
  };
  steps: {
    step: StepId;
    patch: Patch;
    expect: {
      state: PipelineState;
      nextRunnable: StepId | null;
      settled: boolean;
      usable: boolean;
    } | null;
  }[];
}
interface RetryDelayVector {
  kind: 'retry-delay';
  name: string;
  cases: {
    policy: RetryPolicy;
    attempt: number;
    random: number;
    scale: number;
    expect: number | null;
  }[];
}
interface PoliciesVector {
  kind: 'policies';
  name: string;
  expect: unknown;
}
type Vector =
  | SessionVector
  | SessionMatrix
  | PipelineVector
  | RetryDelayVector
  | PoliciesVector;

const files = readdirSync(DIR)
  .filter((f) => f.endsWith('.json'))
  .sort();

const step = (
  s: RecordingState,
  e: LifecycleEvent,
): RecordingState | Rejected => {
  const next = lifecycleReducer(s, e);
  return next ? plain(next) : 'rejected';
};

function compute(v: Vector): Vector {
  switch (v.kind) {
    case 'session': {
      let s =
        v.initial == null
          ? initialRecordingState()
          : RecordingState.parse(v.initial);
      return {
        ...v,
        steps: v.steps.map(({ event }) => {
          const r = step(s, event);
          if (r !== 'rejected') s = r;
          return { event, expect: r };
        }),
      };
    }
    case 'session-matrix': {
      const expectOut: Record<string, (RecordingState | Rejected)[]> = {};
      for (const [name, raw] of Object.entries(v.states)) {
        const s = RecordingState.parse(raw);
        expectOut[name] = v.events.map((e) => step(s, e));
      }
      return { ...v, expect: expectOut };
    }
    case 'pipeline': {
      const i = v.initial;
      let p = newPipeline(i.meetingId, i.stopReason, i.ctx, i.now);
      return {
        ...v,
        steps: v.steps.map(({ step: id, patch }) => {
          p = pipelineStep(p, id, patch);
          return {
            step: id,
            patch,
            expect: {
              state: plain(p),
              nextRunnable: nextRunnableStep(p),
              settled: pipelineSettled(p),
              usable: pipelineUsable(p),
            },
          };
        }),
      };
    }
    case 'retry-delay':
      return {
        ...v,
        cases: v.cases.map((c) => ({
          ...c,
          expect: retryDelay(c.policy, c.attempt, () => c.random, c.scale),
        })),
      };
    case 'policies':
      return { ...v, expect: plain(RETRY_POLICIES) };
  }
}

describe('shared lifecycle vectors (spec/vectors/lifecycle)', () => {
  it('has vectors', () => {
    expect(files.length).toBeGreaterThan(5);
  });
  for (const f of files) {
    it(f, () => {
      const text = readFileSync(`${DIR}${f}`, 'utf8');
      const v = JSON.parse(text) as Vector;
      const actual = compute(v);
      if (UPDATE) {
        writeFileSync(`${DIR}${f}`, `${JSON.stringify(actual, null, 2)}\n`);
        return;
      }
      expect(actual).toEqual(v);
    });
  }
});

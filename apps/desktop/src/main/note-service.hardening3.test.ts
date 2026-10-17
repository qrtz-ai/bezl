/**
 * Hardening cycle 3: NoteService host-side scenarios (real worker handlers, no helper needed).
 * N3 index deadline wiring, N9 template change routed through the pipeline, N12 wall clock jumping back.
 */
import { afterEach, describe, expect, it } from 'vitest';
import {
  AppError,
  EventBus,
  Supervisor,
  createMockHelper,
  defaultConfig,
  newPipeline,
  type Config,
  type FetchLike,
} from '@qrtz/core';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { openDatabase } from '../worker/db';
import { createWorkerHandlers } from '../worker/handlers';
import { NoteService } from './note-service';
import type { WorkerClient } from './worker-client';

const nullLog = {
  debug() {},
  info() {},
  warn() {},
  error() {},
  child() {
    return nullLog;
  },
} as never;

const okReply = () =>
  new Response(
    JSON.stringify({
      choices: [
        {
          message: {
            content: JSON.stringify({ summary: 'AI', actionItems: [], qa: [] }),
          },
        },
      ],
    }),
    { status: 200 },
  );

interface Rpc {
  method: string;
  params: Record<string, unknown> | undefined;
  timeoutMs: number | undefined;
}

const realNow = Date.now;
afterEach(() => {
  Date.now = realNow;
});

function rig(fetch: FetchLike) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-h3-'));
  const h = createWorkerHandlers(openDatabase(':memory:'), { fetch });
  const rpc: Rpc[] = [];
  const worker = {
    async call(method: string, params: unknown, timeoutMs?: number) {
      rpc.push({
        method,
        params: params as Record<string, unknown> | undefined,
        timeoutMs,
      });
      const fn = (h as Record<string, (p: unknown) => unknown>)[method];
      if (!fn) throw new AppError('E_NOT_FOUND', method);
      const r = await fn(
        params === undefined ? undefined : JSON.parse(JSON.stringify(params)),
      );
      return r === undefined ? r : JSON.parse(JSON.stringify(r));
    },
  } as unknown as WorkerClient;
  const base = defaultConfig();
  const config = {
    ...base,
    note: { ...base.note, llmProvider: 'openrouter', autoRecord: false },
  } as Config;
  const bus = new EventBus();
  const note = new NoteService({
    helper: new Supervisor({
      name: 'helper',
      spawn: () => createMockHelper({ scenario: 'standup' }),
    }),
    worker,
    bus,
    log: nullLog,
    secrets: { get: async () => 'sk-test' } as never,
    config: () => config,
    recordingsDir: path.join(dir, 'rec'),
    retryScale: 0.005,
  });
  const meeting = (id: string) => {
    h['note.create']({
      meeting: {
        id,
        title: 'Weekly',
        platform: 'zoom',
        startedAt: 1,
        status: 'processing',
        participants: [],
      },
    });
    h['note.appendLive']({
      meetingId: id,
      segment: {
        id: 's0',
        startMs: 0,
        endMs: 900,
        text: 'We ship on Friday.',
        source: 'live',
      },
    });
  };
  /** A pipeline left unsettled by a "previous run", resumed by recover(). */
  const withPipeline = (id: string) => {
    meeting(id);
    h['pipeline.save']({
      state: newPipeline(
        id,
        'manual',
        {
          startedAt: 1,
          endedAt: 2,
          durationMs: 1,
          audioParts: [],
          active: [],
          helperCrashed: false,
        },
        realNow(),
      ),
      settled: false,
    });
  };
  const settled = (id: string) => {
    const p = h['pipeline.get']({ id });
    return (
      !!p &&
      (['summary', 'index'] as const).every((s) =>
        ['done', 'failed', 'skipped'].includes(p.steps[s].status),
      )
    );
  };
  return { h, note, rpc, bus, meeting, withPipeline, settled };
}

async function until(fn: () => boolean, ms = 4000) {
  const end = realNow() + ms;
  while (realNow() < end) {
    if (fn()) return;
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error('timed out waiting for condition');
}

describe('hardening 3: NoteService', () => {
  it('N3: the index step passes a worker deadline that sits inside its RPC timeout', async () => {
    const r = rig(async () => okReply());
    r.withPipeline('m');
    await r.note.recover();
    await until(() => r.settled('m'));
    const idx = r.rpc.find((c) => c.method === 'note.stepIndex')!;
    expect(idx.params?.deadlineMs).toBeGreaterThan(0);
    expect(idx.params!.deadlineMs as number).toBeLessThan(idx.timeoutMs!);
  });

  it('N9: a template change runs through the pipeline summary step (deadline, queue), not a bare 10 s RPC', async () => {
    const r = rig(async () => okReply());
    r.withPipeline('m');
    await r.note.recover();
    await until(() => r.settled('m'));
    r.rpc.length = 0;
    const s = await r.note.setTemplate('m', 'standup');
    expect(s.template).toBe('standup');
    expect(r.h['note.get']({ id: 'm' }).meeting.template).toBe('standup');
    expect(r.rpc.some((c) => c.method === 'note.resummarize')).toBe(false);
    const step = r.rpc.find(
      (c) => c.method === 'note.stepSummary' && !c.params?.fallback,
    )!;
    expect(step.params?.deadlineMs).toBeLessThan(step.timeoutMs!);
  });

  it('N9: without a pipeline row the direct re-summary is deadline-bounded and saves nothing late', async () => {
    const r = rig(async () => okReply());
    r.meeting('legacy');
    const s = await r.note.setTemplate('legacy', 'sales');
    expect(s.template).toBe('sales');
    const call = r.rpc.find((c) => c.method === 'note.resummarize')!;
    expect(call.timeoutMs).toBeGreaterThan(10_000);
    expect(call.params?.deadlineMs).toBeLessThan(call.timeoutMs!);

    // Worker side: a reply that lands after the deadline keeps the previous summary.
    const late = rig(async () => {
      await new Promise((res) => setTimeout(res, 60));
      return okReply();
    });
    late.meeting('x');
    const err = await late.h['note.resummarize']({
      id: 'x',
      llm: { provider: 'openrouter', apiKey: 'sk' },
      deadlineMs: 15,
    }).catch((e: AppError) => e);
    expect((err as AppError).code).toBe('E_TIMEOUT');
    expect(late.h['note.get']({ id: 'x' }).summary).toBeUndefined();
  });

  it('N12: a wall clock that jumps back an hour while a summary waits to retry does not stall the retry', async () => {
    let calls = 0;
    const r = rig(async () =>
      ++calls === 1
        ? new Response('{"error":{"message":"overloaded"}}', { status: 503 })
        : okReply(),
    );
    let skewed = false;
    r.bus.on('note:pipeline', (v) => {
      if (
        !skewed &&
        Object.values(v.steps).some((s) => s.status === 'retrying')
      ) {
        skewed = true;
        Date.now = () => realNow() - 3_600_000; // NTP / VM resume / manual change
      }
    });
    r.withPipeline('m');
    await r.note.recover();
    await until(() => r.settled('m'), 3000);
    expect(skewed).toBe(true);
    expect(calls).toBe(2);
    expect(r.h['pipeline.get']({ id: 'm' })!.steps.summary.status).toBe('done');
  });
});

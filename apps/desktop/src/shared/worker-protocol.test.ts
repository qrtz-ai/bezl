import { describe, expect, it } from 'vitest';
import { WorkerMessage, WorkerRequest } from './worker-protocol';

describe('worker protocol', () => {
  it('parses every message kind', () => {
    for (const m of [
      { kind: 'log', level: 'info', msg: 'x', fields: { a: 1 } },
      { kind: 'res', id: 1, ok: true, data: 1 },
      {
        kind: 'res',
        id: 2,
        ok: false,
        error: {
          name: 'AppError',
          code: 'E_INTERNAL',
          message: 'm',
          retryable: false,
        },
      },
      { kind: 'heartbeat', ts: 1 },
      { kind: 'ready' },
    ]) {
      expect(WorkerMessage.safeParse(m).success, JSON.stringify(m)).toBe(true);
    }
    expect(
      WorkerRequest.safeParse({ kind: 'req', id: 1, method: 'db.health' })
        .success,
    ).toBe(true);
  });
});

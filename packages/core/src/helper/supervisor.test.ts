import { describe, expect, it } from 'vitest';
import {
  Supervisor,
  backoffDelay,
  type ChildHandle,
  type Clock,
} from './supervisor';
import { createMockHelper } from './mock';
import {
  createLineSplitter,
  encodeHostMessage,
  parseHelperLine,
} from './protocol';

class FakeClock implements Clock {
  t = 0;
  private q: { at: number; fn: () => void; id: number }[] = [];
  private n = 0;
  now = () => this.t;
  setTimeout = (fn: () => void, ms: number) => {
    const id = ++this.n;
    this.q.push({ at: this.t + ms, fn, id });
    return id;
  };
  clearTimeout = (id: unknown) => {
    this.q = this.q.filter((x) => x.id !== id);
  };
  advance(ms: number) {
    const end = this.t + ms;
    for (;;) {
      this.q.sort((a, b) => a.at - b.at);
      const next = this.q[0];
      if (!next || next.at > end) break;
      this.q.shift();
      this.t = next.at;
      next.fn();
    }
    this.t = end;
  }
}

describe('backoffDelay', () => {
  it('grows exponentially and caps', () => {
    const r = () => 0.5;
    expect([0, 1, 2, 3, 10].map((a) => backoffDelay(a, 100, 1000, r))).toEqual([
      100, 200, 400, 800, 1000,
    ]);
  });
});

describe('Supervisor', () => {
  it('runs, restarts after a crash with backoff, and reports status', () => {
    const clock = new FakeClock();
    const kids: ReturnType<typeof createMockHelper>[] = [];
    const sup = new Supervisor({
      name: 'h',
      clock,
      random: () => 0.5,
      baseDelayMs: 100,
      spawn: () => {
        const k = createMockHelper({ clock, heartbeatMs: 1000 });
        kids.push(k);
        return k;
      },
    });
    const states: string[] = [];
    sup.onStatus((s) => states.push(s.state));
    sup.start();
    clock.advance(1);
    expect(sup.status.state).toBe('running');
    kids[0]!.crash(1);
    expect(sup.status.state).toBe('backoff');
    clock.advance(99);
    expect(kids.length).toBe(1);
    clock.advance(2);
    expect(kids.length).toBe(2);
    expect(sup.status).toMatchObject({ state: 'running', restarts: 1 });
    sup.stop();
    expect(states).toEqual([
      'starting',
      'running',
      'backoff',
      'starting',
      'running',
      'stopped',
    ]);
  });

  it('opens the circuit after too many restarts', () => {
    const clock = new FakeClock();
    const sup = new Supervisor({
      name: 'h',
      clock,
      maxRestarts: 3,
      baseDelayMs: 10,
      random: () => 0.5,
      spawn: () => {
        throw new Error('ENOENT');
      },
    });
    sup.start();
    clock.advance(10_000);
    expect(sup.status).toMatchObject({
      state: 'failed',
      restarts: 3,
      lastError: 'ENOENT',
    });
  });

  it('kills a child that stops heartbeating', () => {
    const clock = new FakeClock();
    let killed = 0;
    const silent = (): ChildHandle => ({
      onLine: () => {},
      onExit: () => {},
      write: () => {},
      kill: () => void killed++,
    });
    const sup = new Supervisor({
      name: 'h',
      clock,
      heartbeatTimeoutMs: 1000,
      baseDelayMs: 10,
      random: () => 0.5,
      spawn: silent,
    });
    sup.start();
    clock.advance(1001);
    expect(killed).toBe(1);
    expect(sup.status.lastError).toBe('heartbeat timeout');
  });
});

describe('helper protocol', () => {
  it('parses valid lines and rejects junk', () => {
    expect(parseHelperLine('{"type":"heartbeat","ts":1}')).toEqual({
      ok: true,
      msg: { type: 'heartbeat', ts: 1 },
    });
    expect(parseHelperLine('nope').ok).toBe(false);
    expect(parseHelperLine('{"type":"???"}').ok).toBe(false);
    expect(() => encodeHostMessage({ type: 'bogus' } as never)).toThrow();
  });
  it('splits chunked streams into lines', () => {
    const lines: string[] = [];
    const split = createLineSplitter((l) => lines.push(l));
    split('{"a":1}\n{"b"');
    split(':2}\n');
    expect(lines).toEqual(['{"a":1}', '{"b":2}']);
  });
  it('mock helper answers ping', () => {
    const clock = new FakeClock();
    const h = createMockHelper({ clock });
    const got: string[] = [];
    h.onLine((l) => got.push(l));
    h.write(encodeHostMessage({ type: 'ping', id: 'x' }));
    expect(got.map((l) => JSON.parse(l).type)).toContain('pong');
  });
});

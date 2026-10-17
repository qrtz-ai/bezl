import type { Clock } from './helper/supervisor';

/** Deterministic clock for tests (supervisor, mock helper, session timing). */
export class FakeClock implements Clock {
  t = 0;
  private q: { at: number; fn: () => void; id: number }[] = [];
  private n = 0;
  now = () => this.t;
  setTimeout = (fn: () => void, ms: number) => {
    const id = ++this.n;
    this.q.push({ at: this.t + Math.max(0, ms), fn, id });
    return id;
  };
  clearTimeout = (id: unknown) => {
    this.q = this.q.filter((x) => x.id !== id);
  };
  advance(ms: number) {
    const end = this.t + ms;
    for (;;) {
      this.q.sort((a, b) => a.at - b.at || a.id - b.id);
      const next = this.q[0];
      if (!next || next.at > end) break;
      this.q.shift();
      this.t = next.at;
      next.fn();
    }
    this.t = end;
  }
}

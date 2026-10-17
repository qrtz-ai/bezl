import type { Logger } from '../logger';
import { nullLogger } from '../logger';
import type { HelperStatus } from '../ipc/contract';

/** Minimal handle over a child process so the supervisor is testable without spawning. */
export interface ChildHandle {
  onLine(cb: (line: string) => void): void;
  onExit(cb: (code: number | null, signal: string | null) => void): void;
  write(line: string): void;
  kill(): void;
}

export interface Clock {
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export const systemClock: Clock = {
  now: () => Date.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
};

export interface SupervisorOptions {
  name: string;
  spawn: () => ChildHandle;
  clock?: Clock;
  logger?: Logger;
  random?: () => number;
  baseDelayMs?: number;
  maxDelayMs?: number;
  /** Circuit breaker: more than this many restarts within windowMs → failed. */
  maxRestarts?: number;
  windowMs?: number;
  /** Kill the child if no line (heartbeat or otherwise) arrives within this time. 0 disables. */
  heartbeatTimeoutMs?: number;
}

/** Restart delay: exponential with ±20% jitter, capped. Exported for tests. */
export function backoffDelay(
  attempt: number,
  base: number,
  max: number,
  random: () => number,
): number {
  const exp = Math.min(max, base * 2 ** attempt);
  return Math.round(exp * (0.8 + random() * 0.4));
}

export class Supervisor {
  private child: ChildHandle | null = null;
  private state: HelperStatus['state'] = 'stopped';
  private restartTimes: number[] = [];
  private attempt = 0;
  private lastError: string | undefined;
  private timer: unknown = null;
  private hbTimer: unknown = null;
  private wanted = false;
  private readonly o: Required<Omit<SupervisorOptions, 'spawn' | 'name'>> &
    Pick<SupervisorOptions, 'spawn' | 'name'>;
  private lineListeners = new Set<(line: string) => void>();
  private statusListeners = new Set<(s: HelperStatus) => void>();

  constructor(opts: SupervisorOptions) {
    this.o = {
      clock: systemClock,
      logger: nullLogger,
      random: Math.random,
      baseDelayMs: 500,
      maxDelayMs: 30_000,
      maxRestarts: 5,
      windowMs: 60_000,
      heartbeatTimeoutMs: 15_000,
      ...opts,
    };
  }

  get status(): HelperStatus {
    return {
      state: this.state,
      restarts: this.restartTimes.length,
      ...(this.lastError ? { lastError: this.lastError } : {}),
    };
  }

  onLine(cb: (line: string) => void): () => void {
    this.lineListeners.add(cb);
    return () => this.lineListeners.delete(cb);
  }

  onStatus(cb: (s: HelperStatus) => void): () => void {
    this.statusListeners.add(cb);
    return () => this.statusListeners.delete(cb);
  }

  start(): void {
    this.wanted = true;
    if (this.child || this.state === 'backoff') return;
    this.restartTimes = [];
    this.attempt = 0;
    this.launch();
  }

  stop(): void {
    this.wanted = false;
    this.clearTimers();
    const c = this.child;
    this.child = null;
    c?.kill();
    this.setState('stopped');
  }

  send(line: string): boolean {
    if (!this.child || this.state !== 'running') return false;
    this.child.write(line);
    return true;
  }

  private launch(): void {
    this.setState('starting');
    let child: ChildHandle;
    try {
      child = this.o.spawn();
    } catch (err) {
      this.handleExit(
        null,
        null,
        err instanceof Error ? err.message : String(err),
      );
      return;
    }
    this.child = child;
    child.onLine((line) => {
      if (this.child !== child) return;
      if (this.state === 'starting') {
        this.attempt = 0;
        this.setState('running');
      }
      this.armHeartbeat();
      for (const l of this.lineListeners) l(line);
    });
    child.onExit((code, signal) => {
      if (this.child !== child) return;
      this.child = null;
      this.handleExit(code, signal);
    });
    this.armHeartbeat();
  }

  private handleExit(
    code: number | null,
    signal: string | null,
    error?: string,
  ): void {
    this.clearTimers();
    if (!this.wanted) return this.setState('stopped');
    this.lastError = error ?? `exited code=${code} signal=${signal}`;
    this.o.logger.warn(`${this.o.name} exited`, { code, signal, error });
    const now = this.o.clock.now();
    this.restartTimes = this.restartTimes.filter(
      (t) => now - t < this.o.windowMs,
    );
    if (this.restartTimes.length >= this.o.maxRestarts) {
      this.o.logger.error(`${this.o.name} crash loop; giving up`, {
        restarts: this.restartTimes.length,
      });
      return this.setState('failed');
    }
    this.restartTimes.push(now);
    const delay = backoffDelay(
      this.attempt++,
      this.o.baseDelayMs,
      this.o.maxDelayMs,
      this.o.random,
    );
    this.setState('backoff');
    this.timer = this.o.clock.setTimeout(() => {
      this.timer = null;
      if (this.wanted) this.launch();
    }, delay);
  }

  private armHeartbeat(): void {
    if (!this.o.heartbeatTimeoutMs) return;
    if (this.hbTimer) this.o.clock.clearTimeout(this.hbTimer);
    const child = this.child;
    this.hbTimer = this.o.clock.setTimeout(() => {
      if (this.child !== child || !child) return;
      this.o.logger.warn(`${this.o.name} heartbeat timeout; killing`);
      this.child = null;
      child.kill();
      this.handleExit(null, 'HEARTBEAT', 'heartbeat timeout');
    }, this.o.heartbeatTimeoutMs);
  }

  private clearTimers(): void {
    if (this.timer) this.o.clock.clearTimeout(this.timer);
    if (this.hbTimer) this.o.clock.clearTimeout(this.hbTimer);
    this.timer = this.hbTimer = null;
  }

  private setState(s: HelperStatus['state']): void {
    if (s === this.state) return;
    this.state = s;
    const st = this.status;
    for (const l of this.statusListeners) l(st);
  }
}

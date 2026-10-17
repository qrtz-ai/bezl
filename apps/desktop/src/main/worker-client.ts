import { utilityProcess, type UtilityProcess } from 'electron';
import {
  AppError,
  Supervisor,
  type ChildHandle,
  type Logger,
} from '@qrtz/core';
import { WorkerMessage, type WorkerMethods } from '../shared/worker-protocol';

/** Supervised utilityProcess running DB + heavy jobs, with promise-based RPC. */
export class WorkerClient {
  readonly supervisor: Supervisor;
  private proc: UtilityProcess | null = null;
  private seq = 0;
  private pending = new Map<
    number,
    {
      resolve: (v: unknown) => void;
      reject: (e: unknown) => void;
      t: NodeJS.Timeout;
    }
  >();

  constructor(
    private readonly entry: string,
    private readonly args: string[],
    private readonly log: Logger,
  ) {
    this.supervisor = new Supervisor({
      name: 'worker',
      logger: log,
      heartbeatTimeoutMs: 20_000,
      spawn: () => this.spawn(),
    });
    this.supervisor.onLine((line) => this.onMessage(line));
    this.supervisor.onStatus((s) => {
      if (s.state !== 'running')
        this.failPending(new AppError('E_UNAVAILABLE', `worker ${s.state}`));
    });
  }

  private spawn(): ChildHandle {
    const proc = utilityProcess.fork(this.entry, this.args, {
      serviceName: 'qrtz-worker',
      stdio: 'pipe',
    });
    this.proc = proc;
    proc.stdout?.on('data', (d: Buffer) =>
      this.log.debug('worker stdout', { out: d.toString() }),
    );
    proc.stderr?.on('data', (d: Buffer) =>
      this.log.warn('worker stderr', { out: d.toString().slice(-2000) }),
    );
    return {
      onLine: (cb) =>
        proc.on('message', (m: unknown) =>
          cb(typeof m === 'string' ? m : JSON.stringify(m)),
        ),
      onExit: (cb) => proc.on('exit', (code) => cb(code, null)),
      write: (line) => proc.postMessage(line),
      kill: () => void proc.kill(),
    };
  }

  private onMessage(line: string) {
    let parsed;
    try {
      parsed = WorkerMessage.safeParse(JSON.parse(line));
    } catch {
      return;
    }
    if (!parsed.success) return this.log.warn('bad worker message');
    const m = parsed.data;
    if (m.kind === 'log') this.log[m.level](m.msg, m.fields);
    if (m.kind !== 'res') return;
    const p = this.pending.get(m.id);
    if (!p) return;
    clearTimeout(p.t);
    this.pending.delete(m.id);
    if (m.ok) p.resolve(m.data);
    else p.reject(AppError.fromJSON(m.error));
  }

  private failPending(err: AppError) {
    for (const [id, p] of this.pending) {
      clearTimeout(p.t);
      p.reject(err);
      this.pending.delete(id);
    }
  }

  call<M extends keyof WorkerMethods>(
    method: M,
    params?: WorkerMethods[M]['params'],
    timeoutMs = 10_000,
  ): Promise<WorkerMethods[M]['result']> {
    return new Promise((resolve, reject) => {
      const id = ++this.seq;
      const t = setTimeout(() => {
        this.pending.delete(id);
        reject(new AppError('E_TIMEOUT', `worker ${method} timed out`));
      }, timeoutMs);
      this.pending.set(id, {
        resolve: resolve as (v: unknown) => void,
        reject,
        t,
      });
      if (
        !this.supervisor.send(
          JSON.stringify({ kind: 'req', id, method, params }),
        )
      ) {
        clearTimeout(t);
        this.pending.delete(id);
        reject(new AppError('E_UNAVAILABLE', 'worker not running'));
      }
    });
  }

  start() {
    this.supervisor.start();
  }
  stop() {
    this.supervisor.stop();
    this.proc = null;
  }
}

// Electron utilityProcess entry: owns SQLite and heavy jobs, off the main thread.
import { AppError } from '@qrtz/core';
import { WorkerRequest, type WorkerMessage } from './shared/worker-protocol';
import { dbHealth, openDatabase } from './worker/db';

const port = process.parentPort;
const post = (m: WorkerMessage) => port.postMessage(JSON.stringify(m));
const log = (
  level: 'info' | 'warn' | 'error',
  msg: string,
  fields?: Record<string, unknown>,
) => post({ kind: 'log', level, msg, fields });

const dbFile = process.argv.at(-1) ?? ':memory:';
const db = openDatabase(dbFile);
log('info', 'worker ready', { dbFile });

const methods: Record<string, (params: unknown) => unknown> = {
  'db.health': () => dbHealth(db),
};

port.on('message', async (e: { data: unknown }) => {
  let req;
  try {
    req = WorkerRequest.parse(
      typeof e.data === 'string' ? JSON.parse(e.data) : e.data,
    );
  } catch {
    return log('warn', 'malformed request');
  }
  try {
    const fn = methods[req.method];
    if (!fn)
      throw new AppError('E_NOT_FOUND', `Unknown worker method ${req.method}`);
    post({ kind: 'res', id: req.id, ok: true, data: await fn(req.params) });
  } catch (err) {
    post({
      kind: 'res',
      id: req.id,
      ok: false,
      error: AppError.from(err).toJSON(),
    });
  }
});

post({ kind: 'ready' });
setInterval(() => post({ kind: 'heartbeat', ts: Date.now() }), 5000);

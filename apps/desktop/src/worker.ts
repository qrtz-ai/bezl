// Electron utilityProcess entry: owns SQLite and heavy jobs, off the main thread.
import { AppError } from '@qrtz/core';
import { WorkerRequest, type WorkerMessage } from './shared/worker-protocol';
import { openDatabase } from './worker/db';
import os from 'node:os';
import path from 'node:path';
import { createWorkerHandlers } from './worker/handlers';
import { transformersEmbedder } from './worker/embedder';

const port = process.parentPort;
const post = (m: WorkerMessage) => port.postMessage(JSON.stringify(m));
const log = (
  level: 'info' | 'warn' | 'error',
  msg: string,
  fields?: Record<string, unknown>,
) => post({ kind: 'log', level, msg, fields });

const dbFile = process.argv.at(-1) ?? ':memory:';
const db = openDatabase(dbFile);
const embedder = transformersEmbedder(
  process.env.QRTZ_MODELS_DIR ||
    (dbFile === ':memory:'
      ? os.tmpdir()
      : path.join(path.dirname(dbFile), 'models')),
);
const handlers = createWorkerHandlers(db, {
  fetch: (url, init) => fetch(url, init),
  log: (msg, f) => log('warn', msg, f as Record<string, unknown>),
  embedder,
});
const methods = handlers as unknown as Record<
  string,
  (params: unknown) => unknown
>;
log('info', 'worker ready', { dbFile });

// Packaged-build smoke (scripts/smoke-packaged.mjs): prove the bundled runtime loads. Read-only, no secrets.
if (process.env.QRTZ_EMBED_SELFTEST === '1') {
  const t0 = Date.now();
  void (async () => {
    const e = await embedder();
    const v = await e?.embed(['qrtz embedding self-test']);
    log('info', 'embedder selftest', {
      ok: !!v?.[0],
      backend: e?.backend ?? 'none',
      dims: v?.[0]?.length ?? 0,
      ms: Date.now() - t0,
    });
  })().catch((err: unknown) =>
    log('warn', 'embedder selftest', { ok: false, err: String(err) }),
  );
}

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
    const fn = Object.prototype.hasOwnProperty.call(methods, req.method)
      ? methods[req.method]
      : undefined;
    if (!fn)
      throw new AppError('E_NOT_FOUND', `Unknown worker method ${req.method}`);
    post({ kind: 'res', id: req.id, ok: true, data: await fn(req.params) });
  } catch (err) {
    const e2 = AppError.from(err);
    if (e2.code === 'E_INTERNAL')
      log('error', 'worker method failed', {
        method: req.method,
        err: String(err),
      });
    post({ kind: 'res', id: req.id, ok: false, error: e2.toJSON() });
  }
});

post({ kind: 'ready' });
setInterval(() => post({ kind: 'heartbeat', ts: Date.now() }), 5000);

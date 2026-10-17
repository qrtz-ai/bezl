#!/usr/bin/env node
// Launches the *packaged* app (npm run package first) and checks that the bundled
// transformers.js + onnxruntime-node runtime produces a 384-d embedding in the worker.
// Checks both the native and WASM backends. Linux: run under a display, e.g. `xvfb-run -a npm run smoke:packaged`.
// Models come from QRTZ_MODELS_DIR (default /tmp/qrtz-e2e-models; downloaded once if empty).
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const out = path.join(root, 'apps/desktop/out');
const bin = {
  linux: `qrtz-linux-${process.arch}/qrtz`,
  darwin: `qrtz-darwin-${process.arch}/qrtz.app/Contents/MacOS/qrtz`,
  win32: `qrtz-win32-${process.arch}/qrtz.exe`,
}[process.platform];
const exe = path.join(out, bin);
if (!fs.existsSync(exe)) {
  console.error(`packaged binary not found: ${exe} (run npm run package)`);
  process.exit(1);
}

async function run(backend) {
  const userData = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-smoke-'));
  const logFile = path.join(userData, 'logs/main.jsonl');
  const args = process.platform === 'linux' ? ['--no-sandbox'] : [];
  const child = spawn(exe, args, {
    env: {
      ...process.env,
      QRTZ_USER_DATA: userData,
      QRTZ_EMBED_SELFTEST: '1',
      QRTZ_EMBEDDINGS: '1',
      QRTZ_EMBEDDINGS_BACKEND: backend,
      QRTZ_MODELS_DIR: process.env.QRTZ_MODELS_DIR || '/tmp/qrtz-e2e-models',
    },
    stdio: 'ignore',
  });

  const deadline = Date.now() + 180_000;
  let result;
  while (Date.now() < deadline && !result) {
    await new Promise((r) => setTimeout(r, 500));
    if (!fs.existsSync(logFile)) continue;
    for (const line of fs.readFileSync(logFile, 'utf8').split('\n')) {
      if (!line.includes('embedder selftest')) continue;
      try {
        result = JSON.parse(line);
      } catch {
        /* line still being written */
      }
    }
  }
  child.kill();
  fs.rmSync(userData, { recursive: true, force: true });

  const fields = result?.fields ?? result ?? {};
  console.log(`selftest[${backend}]:`, JSON.stringify(fields));
  return (
    !!result &&
    fields.ok === true &&
    fields.dims === 384 &&
    fields.backend === backend
  );
}

// Native onnxruntime-node, then the onnxruntime-web WASM fallback used on Intel Macs.
const modes =
  process.platform === 'darwin' && process.arch === 'x64'
    ? ['wasm']
    : ['native', 'wasm'];
for (const m of modes) {
  if (!(await run(m))) {
    console.error(
      `FAIL: packaged build did not produce embeddings with the ${m} backend`,
    );
    process.exit(1);
  }
}
console.log(
  `OK: packaged build uses on-device embeddings (${modes.join(' + ')})`,
);

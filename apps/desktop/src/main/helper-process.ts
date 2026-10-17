import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { app } from 'electron';
import {
  createLineSplitter,
  createMockHelper,
  type ChildHandle,
} from '@qrtz/core';

/** Path of the bundled Swift helper (Contents/Resources/qrtz-helper in the .app). */
export function helperBinaryPath(): string {
  return app.isPackaged
    ? path.join(process.resourcesPath, 'qrtz-helper')
    : path.join(
        app.getAppPath(),
        '..',
        '..',
        'native',
        'qrtz-helper',
        '.build',
        'release',
        'qrtz-helper',
      );
}

/** Spawns the real helper on macOS, else (or with QRTZ_MOCK_HELPER=1) the TS mock. */
export function spawnHelper(): ChildHandle {
  const bin = helperBinaryPath();
  if (
    process.platform !== 'darwin' ||
    process.env.QRTZ_MOCK_HELPER === '1' ||
    !fs.existsSync(bin)
  ) {
    return createMockHelper({ os: process.platform });
  }
  const child = spawn(bin, ['--stdio'], { stdio: ['pipe', 'pipe', 'pipe'] });
  return {
    onLine: (cb) => {
      child.stdout.setEncoding('utf8');
      child.stdout.on('data', createLineSplitter(cb));
    },
    onExit: (cb) => {
      child.on('exit', (code, signal) => cb(code, signal));
      child.on('error', () => cb(null, 'ESPAWN'));
    },
    write: (line) => void child.stdin.write(line),
    kill: () => void child.kill('SIGTERM'),
  };
}

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { app } from 'electron';
import {
  HostToHelper,
  createLineSplitter,
  createMockHelper,
  type ChildHandle,
} from '@qrtz/core';
import { writeFixtureWav } from './fixture-audio';

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

export const usingMockHelper = () =>
  process.platform !== 'darwin' ||
  process.env.QRTZ_MOCK_HELPER === '1' ||
  !fs.existsSync(helperBinaryPath());

/**
 * Spawns the real helper on macOS; otherwise the TS mock. QRTZ_MOCK_SCENARIO=standup makes the mock
 * play a scripted meeting (and write fixture audio) — used by the e2e test and Linux dev.
 */
export function spawnHelper(): ChildHandle {
  if (usingMockHelper()) {
    const scenario =
      process.env.QRTZ_MOCK_SCENARIO === 'standup' ? 'standup' : undefined;
    const mock = createMockHelper({
      os: process.platform,
      ...(scenario ? { scenario } : {}),
      speed: Number(process.env.QRTZ_MOCK_SPEED ?? '0.05'),
    });
    return {
      ...mock,
      write(line) {
        const m = HostToHelper.safeParse(JSON.parse(line));
        if (m.success && m.data.type === 'recording.start' && scenario)
          writeFixtureWav(m.data.audioPath);
        mock.write(line);
      },
    };
  }
  const child = spawn(helperBinaryPath(), ['--stdio'], {
    stdio: ['pipe', 'pipe', 'pipe'],
  });
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

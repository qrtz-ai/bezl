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
  // QRTZ_HELPER_BIN points at any build (e.g. the Linux stub) to exercise the real stdio path.
  if (process.env.QRTZ_HELPER_BIN) return process.env.QRTZ_HELPER_BIN;
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
  (process.platform !== 'darwin' && !process.env.QRTZ_HELPER_BIN) ||
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
        const crashAfter = Number(process.env.QRTZ_MOCK_CRASH_AFTER_MS ?? 0);
        if (m.success && m.data.type === 'recording.start' && scenario) {
          const audioPath = m.data.audioPath;
          if (crashAfter > 0) {
            // Fault injection (e2e): the helper dies mid-meeting, leaving a WAV with an unfinalized header
            // holding the first half of the fixture audio.
            setTimeout(() => {
              writeFixtureWav(audioPath, undefined, false);
              const half = fs.readFileSync(audioPath);
              fs.writeFileSync(
                audioPath,
                half.subarray(0, 44 + Math.floor((half.length - 44) / 4) * 2),
              );
              mock.kill();
            }, crashAfter);
          } else writeFixtureWav(audioPath);
        }
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

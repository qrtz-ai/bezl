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

/** What capture backend this run has. `missing` = macOS without the bundled helper (broken install). */
export function helperAvailability(): 'native' | 'simulated' | 'missing' {
  if (process.env.QRTZ_SIMULATE_MISSING_HELPER === '1') return 'missing';
  if (process.env.QRTZ_MOCK_HELPER === '1') return 'simulated';
  if (process.platform === 'darwin' && !fs.existsSync(helperBinaryPath()))
    return 'missing';
  return usingMockHelper() ? 'simulated' : 'native';
}

export const usingMockHelper = () =>
  (process.platform !== 'darwin' && !process.env.QRTZ_HELPER_BIN) ||
  process.env.QRTZ_MOCK_HELPER === '1' ||
  !fs.existsSync(helperBinaryPath());

/**
 * Spawns the real helper on macOS; otherwise the TS mock. QRTZ_MOCK_SCENARIO=standup makes the mock
 * play a scripted meeting (and write fixture audio) — used by the e2e test and Linux dev.
 */
let crashInjected = false;
/** QRTZ_MOCK_REVOKE="microphone@1500,screen-recording@3000": revoke TCC mid-capture (H5 fault injection). */
export function parseRevoke(v: string) {
  return v
    .split(',')
    .map((kv) => kv.trim().split('@'))
    .filter(
      (x): x is ['microphone' | 'screen-recording' | 'accessibility', string] =>
        ['microphone', 'screen-recording', 'accessibility'].includes(
          x[0] ?? '',
        ) && Number.isFinite(Number(x[1])),
    )
    .map(([permission, ms]) => ({ permission, afterMs: Number(ms) }));
}

/** QRTZ_MOCK_PERMISSIONS="screen-recording=denied,microphone=denied" (e2e fault injection). */
function mockPermissions() {
  const out: Record<string, 'granted' | 'denied' | 'unsupported'> = {};
  for (const kv of (process.env.QRTZ_MOCK_PERMISSIONS ?? '').split(',')) {
    const [k, v] = kv.split('=');
    if (k && (v === 'granted' || v === 'denied' || v === 'unsupported'))
      out[k.trim()] = v;
  }
  return out;
}

export function spawnHelper(): ChildHandle {
  if (usingMockHelper()) {
    const scenario =
      process.env.QRTZ_MOCK_SCENARIO === 'standup' ? 'standup' : undefined;
    const mock = createMockHelper({
      os: process.platform,
      ...(scenario ? { scenario } : {}),
      speed: Number(process.env.QRTZ_MOCK_SPEED ?? '0.05'),
      permissions: mockPermissions(),
      ...(process.env.QRTZ_MOCK_REVOKE
        ? { revoke: parseRevoke(process.env.QRTZ_MOCK_REVOKE) }
        : {}),
      ...(['ax', 'paste', 'fail'].includes(process.env.QRTZ_MOCK_INSERT ?? '')
        ? { insert: process.env.QRTZ_MOCK_INSERT as 'ax' | 'paste' | 'fail' }
        : {}),
    });
    return {
      ...mock,
      write(line) {
        const m = HostToHelper.safeParse(JSON.parse(line));
        const crashAfter = Number(process.env.QRTZ_MOCK_CRASH_AFTER_MS ?? 0);
        if (m.success && m.data.type === 'recording.start' && scenario) {
          const audioPath = m.data.audioPath;
          if (crashAfter > 0 && !crashInjected) {
            crashInjected = true; // once per app run: the restarted helper must be able to resume
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

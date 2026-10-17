import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  createLineSplitter,
  encodeHostMessage,
  parseHelperLine,
  type HelperToHost,
} from './protocol';

// Cross-language integration: drives the real Swift binary (Linux stub build or macOS build) over stdio and
// validates every line with zod. Skipped when the helper hasn't been built (`swift build` in native/qrtz-helper).
const root = path.resolve(__dirname, '../../../../native/qrtz-helper/.build');
const bin = ['debug', 'release']
  .map((d) => path.join(root, d, 'qrtz-helper'))
  .find((p) => fs.existsSync(p));

describe.skipIf(!bin)('Swift qrtz-helper over stdio', () => {
  it('handshakes, answers ping, records, and shuts down cleanly', async () => {
    const audioPath = path.join(
      fs.mkdtempSync(path.join(os.tmpdir(), 'qh-')),
      'r.wav',
    );
    const child = spawn(bin!, ['--stdio'], { stdio: ['pipe', 'pipe', 'pipe'] });
    const msgs: HelperToHost[] = [];
    const bad: string[] = [];
    child.stdout.setEncoding('utf8');
    child.stdout.on(
      'data',
      createLineSplitter((l) => {
        const r = parseHelperLine(l);
        if (r.ok) msgs.push(r.msg);
        else bad.push(`${l}: ${r.error}`);
      }),
    );
    for (const m of [
      { type: 'ping', id: 'z' },
      { type: 'permissions.check' },
      { type: 'recording.start', audioPath },
      { type: 'recording.stop' },
      { type: 'shutdown' },
    ] as const)
      child.stdin.write(encodeHostMessage(m));
    const code = await new Promise<number | null>((res) =>
      child.on('exit', res),
    );
    expect(bad).toEqual([]);
    expect(code).toBe(0);
    const types = msgs.map((m) => m.type);
    expect(types[0]).toBe('hello');
    expect(types).toContain('pong');
    expect(types).toContain('recording.started');
    expect(types).toContain('recording.stopped');
    expect(fs.statSync(audioPath).size).toBeGreaterThanOrEqual(44);
  }, 20_000);
});

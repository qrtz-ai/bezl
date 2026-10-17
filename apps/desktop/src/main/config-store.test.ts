import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { ConfigStore } from './config-store';

describe('ConfigStore', () => {
  it('rewrites a pre-rename config.json (memo -> note) once, keeping every value', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-cfg-'));
    const file = path.join(dir, 'config.json');
    fs.writeFileSync(
      file,
      JSON.stringify({
        theme: 'light',
        memo: { autoRecord: true, autoSummarize: false, retainAudioDays: 7 },
      }),
    );
    const store = new ConfigStore(file);
    expect(store.get().note).toMatchObject({
      autoRecord: true,
      autoSummarize: false,
      retainAudioDays: 7,
    });
    const onDisk = JSON.parse(fs.readFileSync(file, 'utf8'));
    expect(onDisk.memo).toBeUndefined();
    expect(onDisk.note.autoRecord).toBe(true);
    expect(onDisk.theme).toBe('light');
    // A current file is left alone.
    const before = fs.statSync(file).mtimeMs;
    new ConfigStore(file);
    expect(fs.statSync(file).mtimeMs).toBe(before);
  });
});

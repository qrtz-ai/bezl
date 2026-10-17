/** Hardening cycle 4 (T3): the talk store holds everything the user dictated, so it is owner-only. */
import { afterEach, describe, expect, it } from 'vitest';
import {
  EventBus,
  MemorySecretStore,
  defaultConfig,
  nullLogger,
} from '@qrtz/core';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { TalkService } from './talk-service';

const dirs: string[] = [];
const tmp = () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-h4-'));
  dirs.push(d);
  return d;
};
afterEach(() => {
  for (const d of dirs.splice(0))
    fs.rmSync(d, { recursive: true, force: true });
});

describe('hardening 4: talk store', () => {
  it('T3: talk history and dictionary are written 0600', () => {
    if (process.platform === 'win32') return;
    const d = tmp();
    const storePath = path.join(d, 'talk.json');
    const svc = new TalkService({
      send: () => {},
      bus: new EventBus(),
      log: nullLogger,
      config: () => defaultConfig(),
      secrets: new MemorySecretStore(),
      storePath,
      ws: (() => {
        throw new Error('no ws');
      }) as never,
      llm: async () => null,
      writeClipboard: () => {},
    } as never);
    svc.setDictionary([{ term: 'qrtz', aliases: ['quartz'] }]);
    expect(fs.statSync(storePath).mode & 0o777).toBe(0o600);
  });
});

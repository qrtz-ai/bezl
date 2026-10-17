/**
 * Hardening cycle 5 (T4): owner-only stores stay owner-only when a crash left a world-readable `.tmp` behind
 * (the write's mode only applies when it creates the file). Covers the talk store and the config store.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  EventBus,
  MemorySecretStore,
  defaultConfig,
  nullLogger,
} from '@qrtz/core';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { ConfigStore } from './config-store';
import { TalkService } from './talk-service';

const dirs: string[] = [];
const tmp = () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-h5-t-'));
  dirs.push(d);
  return d;
};
let umask = 0;
beforeEach(() => {
  umask = process.umask(0o022);
});
afterEach(() => {
  process.umask(umask);
  for (const d of dirs.splice(0))
    fs.rmSync(d, { recursive: true, force: true });
});
const stale = (file: string) => {
  fs.writeFileSync(`${file}.tmp`, 'left by a crash', { mode: 0o644 });
  fs.chmodSync(`${file}.tmp`, 0o644);
};
const mode = (f: string) => fs.statSync(f).mode & 0o777;

describe.runIf(process.platform !== 'win32')(
  'hardening 5: owner-only stores over a crash-left .tmp (T4)',
  () => {
    it('talk history stays 0600', () => {
      const storePath = path.join(tmp(), 'talk.json');
      stale(storePath);
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
      expect(mode(storePath)).toBe(0o600);
      expect(JSON.parse(fs.readFileSync(storePath, 'utf8')).dictionary).toEqual(
        [{ term: 'qrtz', aliases: ['quartz'] }],
      );
    });

    it('config stays 0600', () => {
      const file = path.join(tmp(), 'config.json');
      stale(file);
      new ConfigStore(file).set({ note: { autoRecord: true } });
      expect(mode(file)).toBe(0o600);
    });
  },
);

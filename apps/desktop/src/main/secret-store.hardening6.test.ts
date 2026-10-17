/**
 * Hardening cycle 6 (N30, N33): the encrypted secrets store is written atomically and owner-only. Before, it was
 * rewritten in place: a full disk or a crash mid-write left it empty or cut (every saved key gone, `read()` → `{}`),
 * and an existing 0644 file kept its mode.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

vi.mock('electron', () => ({
  safeStorage: {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => 'gnome_libsecret',
    encryptString: (v: string) => Buffer.from(`enc:${v}`),
    decryptString: (b: Buffer) => b.toString().replace(/^enc:/, ''),
  },
}));

const { SafeStorageSecretStore } = await import('./secret-store');

const dirs: string[] = [];
const tmp = () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-h6-sec-'));
  dirs.push(d);
  return d;
};
let umask = 0;
beforeEach(() => {
  umask = process.umask(0o022);
});
afterEach(() => {
  vi.restoreAllMocks();
  process.umask(umask);
  for (const d of dirs.splice(0))
    fs.rmSync(d, { recursive: true, force: true });
});
const mode = (f: string) => fs.statSync(f).mode & 0o777;

describe.runIf(process.platform !== 'win32')(
  'hardening 6: secrets store is atomic and owner-only (N30, N33)',
  () => {
    it('a disk-full write keeps every saved key; the next write succeeds over the cut .tmp', async () => {
      const file = path.join(tmp(), 'secrets.json');
      const s = new SafeStorageSecretStore(file);
      await s.set('openrouter', 'sk-or-1');
      await s.set('deepgram', 'dg-1');
      const real = fs.writeFileSync;
      const spy = vi
        .spyOn(fs, 'writeFileSync')
        .mockImplementation((p, data, opts) => {
          // A cut write: half the bytes land, then ENOSPC.
          real(p, String(data).slice(0, 10), opts as fs.WriteFileOptions);
          throw Object.assign(new Error('ENOSPC: no space left on device'), {
            code: 'ENOSPC',
          });
        });
      await expect(s.set('anthropic', 'sk-ant')).rejects.toThrow(/ENOSPC/);
      spy.mockRestore();
      expect(await s.get('openrouter')).toBe('sk-or-1');
      expect(await s.get('deepgram')).toBe('dg-1');
      expect(await s.list()).toEqual(
        expect.arrayContaining(['openrouter', 'deepgram']),
      );
      await s.set('anthropic', 'sk-ant');
      expect(await s.get('anthropic')).toBe('sk-ant');
      expect(await s.get('openrouter')).toBe('sk-or-1');
      expect(fs.existsSync(`${file}.tmp`)).toBe(false);
    });

    it('a 0644 store from an older build, and a crash-left 0644 .tmp, end up 0600', async () => {
      const file = path.join(tmp(), 'secrets.json');
      fs.writeFileSync(file, JSON.stringify({}), { mode: 0o644 });
      fs.chmodSync(file, 0o644);
      fs.writeFileSync(`${file}.tmp`, 'left by a crash', { mode: 0o644 });
      fs.chmodSync(`${file}.tmp`, 0o644);
      const s = new SafeStorageSecretStore(file);
      await s.set('openrouter', 'sk-or-1');
      expect(mode(file)).toBe(0o600);
      expect(fs.existsSync(`${file}.tmp`)).toBe(false);
      await s.set('openrouter', null); // a delete rewrites it the same way
      expect(mode(file)).toBe(0o600);
      expect(await s.list()).not.toContain('openrouter');
    });

    it('creates a missing parent directory instead of failing the first save', async () => {
      const file = path.join(tmp(), 'nested', 'secrets.json');
      await new SafeStorageSecretStore(file).set('openrouter', 'sk-or-1');
      expect(mode(file)).toBe(0o600);
    });
  },
);

import fs from 'node:fs';
import path from 'node:path';
import { safeStorage } from 'electron';
import { AppError, envNamesFor, type SecretStore } from '@qrtz/core';

/**
 * Secrets encrypted with Electron safeStorage (macOS Keychain-backed key).
 * File holds only ciphertext. Env vars (QRTZ_SECRET_*) override for headless/dev use.
 */
export class SafeStorageSecretStore implements SecretStore {
  constructor(private readonly file: string) {}

  private read(): Record<string, string> {
    try {
      return JSON.parse(fs.readFileSync(this.file, 'utf8')) as Record<
        string,
        string
      >;
    } catch {
      return {};
    }
  }

  private secure(): boolean {
    if (!safeStorage.isEncryptionAvailable()) return false;
    // On Linux without a keyring Electron falls back to a hard-coded key; refuse that.
    return (
      process.platform !== 'linux' ||
      safeStorage.getSelectedStorageBackend() !== 'basic_text'
    );
  }

  async get(name: string): Promise<string | undefined> {
    const env = envNamesFor(name)
      .map((n) => process.env[n])
      .find(Boolean);
    if (env) return env;
    const enc = this.read()[name];
    if (!enc || !this.secure()) return undefined;
    return safeStorage.decryptString(Buffer.from(enc, 'base64'));
  }

  async set(name: string, value: string | null): Promise<void> {
    const all = this.read();
    if (value === null) delete all[name];
    else {
      if (!this.secure())
        throw new AppError(
          'E_UNAVAILABLE',
          'Secure storage is not available on this system',
        );
      all[name] = safeStorage.encryptString(value).toString('base64');
    }
    this.write(all);
  }

  /**
   * Atomic and owner-only (N30, N33): written to a fresh `.tmp` (a crash-left one is removed first, since an existing
   * file keeps its mode, T4) and renamed over the store. Writing in place truncated the store first, so a crash or a
   * full disk mid-write left it empty or cut and `read()` then returned `{}`: every saved key was gone. It also kept
   * the 0644 mode an older build gave the file. The rename replaces the inode, so the store ends up 0600 either way.
   */
  private write(all: Record<string, string>) {
    fs.mkdirSync(path.dirname(this.file), { recursive: true });
    const tmp = `${this.file}.tmp`;
    fs.rmSync(tmp, { force: true });
    fs.writeFileSync(tmp, JSON.stringify(all), { mode: 0o600 });
    fs.renameSync(tmp, this.file);
  }

  async list(): Promise<string[]> {
    const names = new Set(Object.keys(this.read()));
    for (const k of Object.keys(process.env))
      if (k.startsWith('QRTZ_SECRET_')) names.add(k);
    return [...names].sort();
  }
}

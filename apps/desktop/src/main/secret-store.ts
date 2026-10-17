import fs from 'node:fs';
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
    fs.writeFileSync(this.file, JSON.stringify(all), { mode: 0o600 });
  }

  async list(): Promise<string[]> {
    const names = new Set(Object.keys(this.read()));
    for (const k of Object.keys(process.env))
      if (k.startsWith('QRTZ_SECRET_')) names.add(k);
    return [...names].sort();
  }
}

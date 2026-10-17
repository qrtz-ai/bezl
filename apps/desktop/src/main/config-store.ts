import fs from 'node:fs';
import path from 'node:path';
import {
  applyConfigPatch,
  loadConfig,
  migrateLegacyConfig,
  type Config,
} from '@qrtz/core';

/** JSON config persisted atomically (tmp + rename). */
export class ConfigStore {
  private cfg: Config;
  private listeners = new Set<(c: Config) => void>();
  constructor(private readonly file: string) {
    let raw: unknown;
    try {
      raw = JSON.parse(fs.readFileSync(file, 'utf8'));
    } catch {
      raw = undefined;
    }
    this.cfg = loadConfig(raw);
    // Renamed keys (memo -> note): rewrite the file once so it only ever holds current keys.
    if (migrateLegacyConfig(raw).migrated) this.persist();
  }
  get(): Config {
    return this.cfg;
  }
  set(patch: unknown): Config {
    this.cfg = applyConfigPatch(this.cfg, patch);
    this.persist();
    for (const l of this.listeners) l(this.cfg);
    return this.cfg;
  }
  private persist() {
    fs.mkdirSync(path.dirname(this.file), { recursive: true });
    const tmp = `${this.file}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify(this.cfg, null, 2), { mode: 0o600 });
    fs.renameSync(tmp, this.file);
  }
  onChange(cb: (c: Config) => void) {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }
}

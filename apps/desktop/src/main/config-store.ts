import fs from 'node:fs';
import path from 'node:path';
import { applyConfigPatch, loadConfig, type Config } from '@qrtz/core';

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
  }
  get(): Config {
    return this.cfg;
  }
  set(patch: unknown): Config {
    this.cfg = applyConfigPatch(this.cfg, patch);
    fs.mkdirSync(path.dirname(this.file), { recursive: true });
    const tmp = `${this.file}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify(this.cfg, null, 2), { mode: 0o600 });
    fs.renameSync(tmp, this.file);
    for (const l of this.listeners) l(this.cfg);
    return this.cfg;
  }
  onChange(cb: (c: Config) => void) {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }
}

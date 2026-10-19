import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';

const repo = path.join(import.meta.dirname, '../..');
const read = (p) => fs.readFileSync(path.join(repo, p), 'utf8');

describe('dev builds are TCC-stable (docs/dev-permissions.md)', () => {
  it('the src-tauri cargo runner points at scripts/dev/run.sh', () => {
    const cfg = read('apps/desktop/src-tauri/.cargo/config.toml');
    const runner = /runner = "([^"]+)"/.exec(cfg)?.[1];
    expect(runner).toBeDefined();
    const file = path.resolve(repo, 'apps/desktop/src-tauri', runner);
    expect(file).toBe(path.join(repo, 'scripts/dev/run.sh'));
    expect(fs.statSync(file).mode & 0o111).not.toBe(0);
  });

  it('helpers keep the release identifiers; the app has its own dev id', () => {
    const dev = read('scripts/dev/sign.sh');
    const rel = read('scripts/release/sign-mac.sh');
    for (const [name, id] of [['qrtz', 'ai.qrtz.cli']]) {
      expect(dev).toContain(`\nsign ${name} ${id}\n`);
      expect(rel).toMatch(
        new RegExp(`\\n${name} ${id.replace(/\./g, '\\.')} `),
      );
    }
    expect(dev).toContain('\nsign qrtz-app ai.qrtz.desktop.dev\n');
    // ADR 0169: no module code in the app; a dev monolith signs as ai.qrtz.helper, never as a module.
    expect(rel).not.toMatch(/\nqrtz-(helper|snap|echo|nano|ctrl) /);
    expect(dev).toContain('ai.qrtz.helper');
  });

  it('runs anything that is not qrtz-app untouched', () => {
    const r = spawnSync(
      path.join(repo, 'scripts/dev/run.sh'),
      ['/bin/echo', 'ok'],
      { encoding: 'utf8' },
    );
    expect(r.status).toBe(0);
    expect(r.stdout).toBe('ok\n');
  });
});

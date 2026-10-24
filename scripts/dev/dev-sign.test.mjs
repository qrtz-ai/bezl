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
    for (const [name, id] of [
      ['qrtz', 'ai.qrtz.cli'],
      ['qrtz-echo', 'ai.qrtz.echo'],
      ['qrtz-nano', 'ai.qrtz.nano'],
      ['qrtz-ctrl', 'ai.qrtz.ctrl'],
      ['qrtz-snap', 'ai.qrtz.snap'],
    ]) {
      expect(dev).toContain(`\nsign ${name} ${id}\n`);
      if (name === 'qrtz')
        expect(rel).toMatch(
          new RegExp(`\\n${name} ${id.replace(/\./g, '\\.')} `),
        );
    }
    // QRT-255: release signs bundled helpers from module.json, not a hard-coded table.
    // ADR 0237: all four first-party helpers are bundled.
    for (const id of ['echo', 'nano', 'ctrl', 'snap']) {
      const m = JSON.parse(read(`modules/${id}/module.json`));
      expect(m.launch).toMatchObject({
        kind: 'bundled-helper',
        entry: `qrtz-${id}`,
      });
      expect(m.code.bundleId).toBe(`ai.qrtz.${id}`);
      expect(rel).not.toContain(`ai.qrtz.${id}`);
    }
    expect(rel).toContain('bundled_helpers "$repo/modules"');
    expect(dev).toContain('\nsign qrtz-app ai.qrtz.desktop.dev\n');
    // The release NESTED table never lists module helpers (they come from module.json); a dev monolith signs as
    // ai.qrtz.helper.
    expect(rel).not.toMatch(/\nqrtz-(helper|snap|nano|ctrl) /);
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
  it('only shell-script stubs are skipped, never a Mach-O that mentions the stub mark', () => {
    const dev = read('scripts/dev/sign.sh');
    const line = dev
      .split('\n')
      .find((l) => l.includes('qrtz: dev stub') && l.includes('return 0'));
    expect(line).toBeDefined();
    expect(line).toContain("'#!'");
  });
});

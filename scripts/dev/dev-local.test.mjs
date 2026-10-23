// dev:local (scripts/dev/dev-local.sh): flag checks run before anything is built, the script never writes to
// /Applications or uses a release build, and the release paths refuse it. The build and launch need macOS (run
// `bun run dev:local -- --build-only` on the Mac).
import { describe, expect, it } from 'vitest';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import path from 'node:path';

const root = path.join(import.meta.dirname, '..', '..');
const script = path.join(import.meta.dirname, 'dev-local.sh');
const read = (p) => readFileSync(path.join(root, p), 'utf8');
const run = (...args) =>
  spawnSync('bash', [script, ...args], {
    encoding: 'utf8',
    env: { ...process.env, PATH: process.env.PATH },
  });

describe('dev-local.sh', () => {
  it('is wired as bun run dev:local', () => {
    expect(JSON.parse(read('package.json')).scripts['dev:local']).toBe(
      'bash scripts/dev/dev-local.sh',
    );
  });

  it('refuses an unknown module id for --without, and an unknown flag', () => {
    let r = run('--without', 'bogus');
    expect(r.status).toBe(1);
    expect(r.stderr).toContain(
      '--without bogus: not one of nano snap ctrl echo',
    );
    r = run('--without');
    expect(r.status).toBe(1);
    expect(r.stderr).toContain('--without needs an id');
    r = run('--release');
    expect(r.status).toBe(1);
    expect(r.stderr).toContain('unknown flag --release');
  });

  it('gets past flag parsing with no --without (bash 3.2 on macOS: an empty array under set -u)', () => {
    // Off macOS no args would exec plain tauri dev, so only --build-only runs there.
    for (const args of [
      ...(process.platform === 'darwin' ? [[]] : []),
      ['--build-only'],
      ['--without', 'ctrl', '--build-only'],
    ]) {
      const r = run(...args);
      expect(r.stderr).not.toContain('unbound variable');
      if (process.platform !== 'darwin')
        expect(r.stderr).toContain('not macOS');
    }
  });

  it('runs all four modules by default and sets each QRTZ_<ID>_BIN and a separate QRTZ_USER_DATA', () => {
    const s = read('scripts/dev/dev-local.sh');
    expect(s).toContain('ids=(nano snap ctrl echo)');
    expect(s).toContain("QRTZ_$(tr '[:lower:]' '[:upper:]' <<<\"$id\")_BIN");
    expect(s).toContain('ai.qrtz.desktop-dev');
    expect(s).toMatch(/is the real data dir or inside it: refused/);
  });

  it('never builds release and never writes into /Applications', () => {
    const code = read('scripts/dev/dev-local.sh')
      .split('\n')
      .filter(
        (l) =>
          !l.trimStart().startsWith('#') &&
          !l.includes('[dev-local]') &&
          !/\bsay\b/.test(l),
      )
      .join('\n');
    expect(code).not.toMatch(/--release|-c release/);
    expect(code).not.toMatch(
      /(cp|mv|rm|ditto|rsync|ln|install)\b[^\n]*\/Applications/,
    );
    expect(code).toContain('--features dev-local');
  });

  it('the dev-local feature is off by default and release paths refuse it', () => {
    for (const t of [
      'apps/desktop/src-tauri/Cargo.toml',
      'crates/qrtz-app/Cargo.toml',
    ]) {
      expect(read(t)).not.toMatch(/^default *= *\[[^\]]*dev-local/m);
      expect(read(t)).toMatch(/^dev-local *=/m);
    }
    expect(read('scripts/app/publish-local.sh')).toContain(
      'dev-local is enabled in this shell',
    );
    expect(read('scripts/release/verify-bundle.sh')).toContain(
      'contains dev-local override strings',
    );
  });
});

// Bozniak's 23d892af review: a real dev bundle id, and the release guard covers echo and the legacy overrides.
describe('dev-local review (23d892af)', () => {
  it('launches as ai.qrtz.desktop.dev, not the production id', () => {
    const s = read('scripts/dev/dev-local.sh');
    expect(s).toContain(
      `tauri dev --features dev-local --config '{"identifier":"ai.qrtz.desktop.dev"}'`,
    );
  });
  it('verify-bundle scans qrtz-echo too and refuses QRTZ_MODULE_HELPERS / QRTZ_HELPER_BIN', () => {
    const v = read('scripts/release/verify-bundle.sh');
    expect(v).toContain(
      'for bin in "$macos/qrtz-app" "$macos/qrtz" $(helper_names | sed "s|^|$macos/|"); do',
    );
    expect(v).toContain(
      'QRTZ_(NANO|SNAP|CTRL|ECHO|HELPER)_BIN|QRTZ_MODULE_HELPERS|QRTZ_DEV_REPO|dev-local: ',
    );
  });
});

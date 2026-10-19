// scripts/app/version.mjs: qrtz.app's patch bumps against `app-v<x.y.z>` tags, every version file, CHANGELOG, tags.
import { describe, expect, it } from 'vitest';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  applyBump,
  changelogWith,
  lastTag,
  plan,
  tag,
  workspaceMembers,
} from './version.mjs';

const g = (repo, ...a) =>
  execFileSync('git', a, { cwd: repo, encoding: 'utf8' }).trim();
const w = (repo, f, s) => {
  fs.mkdirSync(path.dirname(path.join(repo, f)), { recursive: true });
  fs.writeFileSync(path.join(repo, f), s);
};
const r = (repo, f) => fs.readFileSync(path.join(repo, f), 'utf8');
const commit = (repo, msg) => {
  g(repo, 'add', '-A');
  g(repo, 'commit', '-qm', msg);
};

function fixture() {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-appver-'));
  g(repo, 'init', '-q', '-b', 'main');
  g(repo, 'config', 'user.email', 't@t');
  g(repo, 'config', 'user.name', 't');
  w(
    repo,
    'apps/desktop/src-tauri/tauri.conf.json',
    '{\n  "productName": "qrtz",\n  "version": "0.2.0"\n}\n',
  );
  w(repo, 'package.json', '{\n  "name": "qrtz",\n  "version": "0.2.0"\n}\n');
  w(
    repo,
    'apps/desktop/package.json',
    '{\n  "name": "@qrtz/desktop",\n  "version": "0.2.0"\n}\n',
  );
  w(
    repo,
    'Cargo.toml',
    '[workspace]\nmembers = ["crates/a"]\n\n[workspace.package]\nversion = "0.2.0"\nedition = "2021"\n',
  );
  w(
    repo,
    'crates/a/Cargo.toml',
    '[package]\nname = "qrtz-a"\nversion.workspace = true\n',
  );
  w(repo, 'crates/a/src/lib.rs', '1\n');
  w(
    repo,
    'Cargo.lock',
    '[[package]]\nname = "qrtz-a"\nversion = "0.2.0"\n\n[[package]]\nname = "serde"\nversion = "0.2.0"\n',
  );
  w(repo, 'modules/echo/qrtz.json', '{"version": "0.2.0"}\n');
  w(repo, 'native/qrtz-helper/Sources/QrtzNano/a.swift', '1\n');
  w(repo, 'native/qrtz-helper/Sources/QrtzEcho/a.swift', '1\n');
  w(
    repo,
    'CHANGELOG.md',
    '# Changelog\n\n## Unreleased\n\n- **faster echo:** it is.\n\n## 0.2.0\n\n- first\n',
  );
  commit(repo, 'init');
  g(repo, 'tag', 'v0.2.0');
  return repo;
}

describe('scripts/app/version.mjs (ADR 0181)', () => {
  it('an old v tag is the last release; nothing changed → no bump', () => {
    const repo = fixture();
    expect(lastTag(repo)).toEqual({ tag: 'v0.2.0', version: '0.2.0' });
    expect(plan(repo).to).toBeNull();
    expect(plan(repo, true).to).toBe('0.2.1');
  });

  it('nano, ctrl and snap code and docs never bump the shell; echo and the app do', () => {
    const repo = fixture();
    w(repo, 'native/qrtz-helper/Sources/QrtzNano/a.swift', '2\n');
    w(repo, 'docs/x.md', 'y\n');
    w(repo, 'crates/a/README.md', 'y\n');
    commit(repo, 'nano only');
    expect(plan(repo).to).toBeNull();
    // echo is inside qrtz.app (ADR 0174): its code changes only with the whole signed bundle.
    w(repo, 'native/qrtz-helper/Sources/QrtzEcho/a.swift', '2\n');
    commit(repo, 'echo: tweak');
    expect(plan(repo)).toMatchObject({
      from: '0.2.0',
      to: '0.2.1',
      since: 'v0.2.0',
    });
  });

  it('bump writes every version file, only the workspace lock entries, and the changelog', () => {
    const repo = fixture();
    w(repo, 'crates/a/src/lib.rs', '2\n');
    commit(repo, 'app: a fix');
    expect(workspaceMembers(repo)).toEqual(['qrtz-a']);
    const files = applyBump(repo, plan(repo), '2026-10-19');
    expect(files).toContain('Cargo.lock');
    expect(
      JSON.parse(r(repo, 'apps/desktop/src-tauri/tauri.conf.json')).version,
    ).toBe('0.2.1');
    expect(JSON.parse(r(repo, 'package.json')).version).toBe('0.2.1');
    expect(JSON.parse(r(repo, 'apps/desktop/package.json')).version).toBe(
      '0.2.1',
    );
    expect(r(repo, 'Cargo.toml')).toContain('version = "0.2.1"');
    expect(r(repo, 'Cargo.lock')).toContain(
      'name = "qrtz-a"\nversion = "0.2.1"',
    );
    expect(r(repo, 'Cargo.lock')).toContain(
      'name = "serde"\nversion = "0.2.0"',
    );
    const cl = r(repo, 'CHANGELOG.md');
    expect(cl).toMatch(
      /^# Changelog\n\n## Unreleased\n\n## 0\.2\.1 \(2026-10-19\)\n\n- \*\*faster echo:\*\* it is\.\n\n## 0\.2\.0\n/,
    );
    // Already ahead of its tag: not bumped twice.
    commit(repo, 'release 0.2.1');
    expect(plan(repo).to).toBeNull();
    expect(plan(repo).reason).toContain('already ahead');
  });

  it('with no unreleased notes the commit subjects are the section', () => {
    const out = changelogWith(
      '# Changelog\n\n## Unreleased\n\n## 0.2.0\n\n- a\n',
      '0.2.1',
      '2026-10-19',
      ['abc1234 echo: catch the first word'],
    );
    expect(out).toContain(
      '## 0.2.1 (2026-10-19)\n\n- echo: catch the first word (abc1234)\n',
    );
    expect(out.indexOf('## Unreleased')).toBeLessThan(out.indexOf('## 0.2.1'));
  });

  it('tag makes app-v<version> at HEAD and refuses one elsewhere', () => {
    const repo = fixture();
    w(repo, 'crates/a/src/lib.rs', '2\n');
    commit(repo, 'x');
    applyBump(repo, plan(repo), '2026-10-19');
    commit(repo, 'release');
    expect(tag(repo)).toBe('app-v0.2.1');
    expect(tag(repo)).toBeNull();
    expect(lastTag(repo)).toEqual({ tag: 'app-v0.2.1', version: '0.2.1' });
    w(repo, 'crates/a/src/lib.rs', '3\n');
    commit(repo, 'y');
    expect(() => tag(repo)).toThrow(/exists at/);
  });
});

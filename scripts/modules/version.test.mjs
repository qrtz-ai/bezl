// version.mjs: per-module patch bumps against `<m>-v<x.y.z>` tags, lock hash, CHANGELOG, tags.
import { describe, expect, it } from 'vitest';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  applyBump,
  bumpPatch,
  changelogEntry,
  cmpVersion,
  lastTag,
  planOne,
  tagAll,
} from './version.mjs';

const g = (repo, ...a) =>
  execFileSync('git', a, { cwd: repo, encoding: 'utf8' }).trim();
const w = (repo, f, s) => {
  fs.mkdirSync(path.dirname(path.join(repo, f)), { recursive: true });
  fs.writeFileSync(path.join(repo, f), s);
};

function fixture() {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-ver-'));
  g(repo, 'init', '-q', '-b', 'main');
  g(repo, 'config', 'user.email', 't@t');
  g(repo, 'config', 'user.name', 't');
  const lock = { lockVersion: 1, modules: {} };
  for (const [m, v] of [
    ['nano', '0.2.0'],
    ['ctrl', '0.2.0'],
    ['snap', '0.1.0'],
  ]) {
    w(
      repo,
      `modules/${m}/qrtz.json`,
      `{\n  "name": "${m}",\n  "version": "${v}",\n  "extension": { "minApp": "0.2.0" }\n}\n`,
    );
    w(
      repo,
      `modules/${m}/module.json`,
      `{\n  "id": "${m}",\n  "version": "${v}",\n  "minApp": "0.2.0"\n}\n`,
    );
    lock.modules[m] = { manifestSha256: 'x', version: v };
  }
  w(repo, 'modules/modules.lock.json', JSON.stringify(lock, null, 2) + '\n');
  w(repo, 'native/qrtz-helper/Sources/QrtzCtrl/a.swift', '1\n');
  w(repo, 'native/qrtz-helper/Sources/QrtzSnap/a.swift', '1\n');
  w(repo, 'native/qrtz-helper/Sources/QrtzNano/a.swift', '1\n');
  w(repo, 'README', 'x\n');
  g(repo, 'add', '-A');
  g(repo, 'commit', '-qm', 'init');
  for (const t of ['nano-v0.2.0', 'ctrl-v0.2.0', 'snap-v0.1.0'])
    g(repo, 'tag', t);
  return repo;
}

describe('version.mjs', () => {
  it('compares and bumps plain versions', () => {
    expect(bumpPatch('0.2.9')).toBe('0.2.10');
    expect(cmpVersion('0.2.10', '0.2.9')).toBeGreaterThan(0);
    expect(() => bumpPatch('1.0.0-beta')).toThrow();
  });

  it('bumps only modules whose code or manifest changed since their tag', () => {
    const repo = fixture();
    w(repo, 'native/qrtz-helper/Sources/QrtzCtrl/a.swift', '2\n');
    w(repo, 'README', 'y\n');
    g(repo, 'commit', '-qam', 'ctrl: screensaver');
    expect(planOne(repo, 'ctrl')).toMatchObject({
      from: '0.2.0',
      to: '0.2.1',
      reason: 'changed',
    });
    expect(planOne(repo, 'snap').to).toBeNull();
    expect(planOne(repo, 'snap', true)).toMatchObject({
      to: '0.1.1',
      reason: 'forced',
    });
  });

  it('a shared helper change bumps every module', () => {
    const repo = fixture();
    w(repo, 'native/qrtz-helper/Sources/QrtzShared/s.swift', '1\n');
    g(repo, 'add', '-A');
    g(repo, 'commit', '-qm', 'shared');
    for (const m of ['nano', 'ctrl', 'snap'])
      expect(planOne(repo, m).to).not.toBeNull();
  });

  it('writes both manifests, the lock entry with module.json’s hash, and a CHANGELOG from the subjects', () => {
    const repo = fixture();
    w(repo, 'native/qrtz-helper/Sources/QrtzCtrl/a.swift', '2\n');
    g(repo, 'commit', '-qam', 'ctrl: screensaver');
    applyBump(repo, planOne(repo, 'ctrl'), '2026-10-18');
    const q = JSON.parse(
      fs.readFileSync(path.join(repo, 'modules/ctrl/qrtz.json'), 'utf8'),
    );
    expect(q.version).toBe('0.2.1');
    expect(q.extension.minApp).toBe('0.2.0');
    const mj = fs.readFileSync(path.join(repo, 'modules/ctrl/module.json'));
    expect(JSON.parse(mj).version).toBe('0.2.1');
    const lock = JSON.parse(
      fs.readFileSync(path.join(repo, 'modules/modules.lock.json'), 'utf8'),
    );
    expect(lock.modules.ctrl).toEqual({
      version: '0.2.1',
      manifestSha256: createHash('sha256').update(mj).digest('hex'),
    });
    expect(lock.modules.snap.version).toBe('0.1.0');
    const cl = fs.readFileSync(
      path.join(repo, 'modules/ctrl/CHANGELOG.md'),
      'utf8',
    );
    expect(cl).toMatch(
      /^# ctrl changelog\n\n## 0\.2\.1 \(2026-10-18\)\n\n- ctrl: screensaver \([0-9a-f]{7}\)\n/,
    );
  });

  it('does not bump twice before the release is tagged, then tags and refuses a moved tag', () => {
    const repo = fixture();
    applyBump(repo, planOne(repo, 'snap', true), '2026-10-18');
    g(repo, 'commit', '-qam', 'snap 0.1.1');
    expect(planOne(repo, 'snap', true)).toMatchObject({
      to: null,
      reason: 'already ahead of snap-v0.1.0',
    });
    expect(tagAll(repo, ['snap'])).toEqual(['snap-v0.1.1']);
    expect(lastTag(repo, 'snap')).toEqual({
      tag: 'snap-v0.1.1',
      version: '0.1.1',
    });
    w(repo, 'native/qrtz-helper/Sources/QrtzSnap/a.swift', '2\n');
    g(repo, 'commit', '-qam', 'snap change');
    expect(() => tagAll(repo, ['snap'])).toThrow(/different code/);
  });

  it('formats an entry with no changes', () => {
    expect(changelogEntry('snap', '0.1.1', [], '2026-10-18')).toBe(
      '## 0.1.1 (2026-10-18)\n\n- no code changes; republished\n',
    );
  });
});

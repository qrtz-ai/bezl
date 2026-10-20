// QRT-275: source-archives.sh writes the qrtz source at the release commit, FluidAudio at exactly the pinned revision
// and the build scripts, with their sha256s; it refuses a FluidAudio fetch that isn't the pinned revision.
import { describe, expect, it } from 'vitest';
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '../..');
const script = path.join(import.meta.dirname, 'source-archives.sh');
const git = (cwd, ...a) =>
  execFileSync('git', a, { cwd, encoding: 'utf8' }).trim();

describe('source-archives.sh', () => {
  it('writes the three archives and their sums from a local FluidAudio at the pinned revision', () => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'sa-'));
    // A stand-in FluidAudio repo; pin its commit in a throwaway qrtz repo.
    const fa = path.join(tmp, 'fa');
    fs.mkdirSync(fa);
    git(fa, 'init', '-q');
    fs.writeFileSync(path.join(fa, 'LICENSE'), 'Apache');
    git(fa, 'add', '-A');
    git(fa, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', 'fa');
    git(fa, 'config', 'uploadpack.allowReachableSHA1InWant', 'true');
    const rev = git(fa, 'rev-parse', 'HEAD');
    const q = path.join(tmp, 'q');
    fs.mkdirSync(path.join(q, 'scripts/release'), { recursive: true });
    fs.mkdirSync(path.join(q, 'native/qrtz-helper'), { recursive: true });
    fs.copyFileSync(script, path.join(q, 'scripts/release/source-archives.sh'));
    fs.writeFileSync(
      path.join(q, 'native/qrtz-helper/Package.swift'),
      'exact: "0.17.7"\n',
    );
    fs.writeFileSync(
      path.join(q, 'native/qrtz-helper/Package.resolved'),
      JSON.stringify({
        pins: [
          {
            identity: 'fluidaudio',
            location: 'https://example.invalid/fa.git',
            state: { revision: rev, version: '0.17.7' },
          },
        ],
      }),
    );
    fs.writeFileSync(path.join(q, 'package.json'), '{}');
    git(q, 'init', '-q');
    git(q, 'add', '-A');
    git(q, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', 'q');
    const out = path.join(tmp, 'out');
    const r = spawnSync(
      'bash',
      [path.join(q, 'scripts/release/source-archives.sh'), '0.2.5', out],
      {
        encoding: 'utf8',
        env: { ...process.env, QRTZ_FLUIDAUDIO_GIT: `file://${fa}` },
      },
    );
    expect(r.status, r.stderr).toBe(0);
    const sums = fs
      .readFileSync(path.join(out, 'qrtz_0.2.5_source.sha256'), 'utf8')
      .trim()
      .split('\n');
    expect(sums.map((l) => l.split('  ')[1])).toEqual([
      'qrtz_0.2.5_source.tar.gz',
      'qrtz_0.2.5_fluidaudio_source.tar.gz',
      'qrtz_0.2.5_build-scripts.tar.gz',
    ]);
    const list = execFileSync(
      'tar',
      ['-tzf', path.join(out, 'qrtz_0.2.5_fluidaudio_source.tar.gz')],
      { encoding: 'utf8' },
    );
    expect(list).toContain('FluidAudio-0.17.7/LICENSE');
    // A pin that Package.swift doesn't match exactly is refused.
    fs.writeFileSync(
      path.join(q, 'native/qrtz-helper/Package.swift'),
      'from: "0.17.7"\n',
    );
    git(
      q,
      '-c',
      'user.name=t',
      '-c',
      'user.email=t@t',
      'commit',
      '-qam',
      'loose',
    );
    const bad = spawnSync(
      'bash',
      [path.join(q, 'scripts/release/source-archives.sh'), '0.2.5', out],
      {
        encoding: 'utf8',
        env: { ...process.env, QRTZ_FLUIDAUDIO_GIT: `file://${fa}` },
      },
    );
    expect(bad.status).not.toBe(0);
    expect(bad.stderr).toContain('exact');
  });
  it('the repo pins FluidAudio exactly, as the script requires', () => {
    expect(
      fs.readFileSync(
        path.join(root, 'native/qrtz-helper/Package.swift'),
        'utf8',
      ),
    ).toContain('exact: "0.17.7"');
  });
});

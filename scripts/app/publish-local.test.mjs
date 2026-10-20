// ADR 0181: the Mac's qrtz.app publish (scripts/app/publish-local.sh), checked on any OS through its dry run against a
// throwaway clone (real git, a local bare origin) with curl and security stubbed. Plus gh-store.sh upload-app's order
// (latest.json last) with gh stubbed (QRT-297: the index GitHub release). Nothing here signs or uploads anything.
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const real = path.resolve(import.meta.dirname, '../..');
const COPY = [
  'scripts/app/publish-local.sh',
  'scripts/app/feed.mjs',
  'scripts/app/version.mjs',
  'scripts/app/sign-feed.sh',
  'scripts/release/gh-store.sh',
  'scripts/app/verify-live.sh',
  'scripts/modules/version.mjs',
  'crates/qrtz-app/src/app_update_key.pub',
];
const KEY = 'UNTRUSTEDSECRETKEYSENTINEL0123456789abcdef';
const GHTOKEN = 'GHTOKENSENTINEL9876543210';
const V = '0.2.1';

let tmp, repo, bin, log, key, target, gitEnv;

const git = (args, cwd = repo) =>
  execFileSync('git', args, { cwd, env: gitEnv, encoding: 'utf8' }).trim();

// curl: v1.app.latest.json from $STUB_FEED (404 without it); the v1.app.<v>. archive 200 with STUB_ARCHIVE_LIVE=1.
const CURL = `#!/usr/bin/env bash
echo "curl $*" >>"$STUB_LOG"
out="" url="" w=""
while [[ $# -gt 0 ]]; do
  case "$1" in -o) out="$2"; shift 2 ;; -w) w="$2"; shift 2 ;; -H|--max-time) shift 2 ;; -*) shift ;; *) url="$1"; shift ;; esac
done
code=404
case "$url" in
  */v1.app.latest.json) [[ -n "\${STUB_FEED:-}" ]] && { code=200; [[ -n "$out" ]] && cp "$STUB_FEED" "$out"; } ;;
  */v1.app.*.qrtz_*_aarch64.app.tar.gz) [[ "\${STUB_ARCHIVE_LIVE:-}" == 1 ]] && code=200 ;;
esac
[[ -n "$w" ]] && printf '%s' "$code"
exit 0
`;
const LOGGER = (name) =>
  `#!/usr/bin/env bash\necho "${name} $*" >>"$STUB_LOG"\nexit 0\n`;

function envelope(version) {
  const feed = Buffer.from(
    JSON.stringify({ version, expires: '2026-11-18T00:00:00Z' }),
  );
  const f = path.join(tmp, `live-${version}.json`);
  fs.writeFileSync(
    f,
    JSON.stringify({ feed: feed.toString('base64'), signature: 'c2ln' }),
  );
  return f;
}

beforeEach(() => {
  tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'publish-local-'));
  repo = path.join(tmp, 'qrtz');
  bin = path.join(tmp, 'bin');
  log = path.join(tmp, 'stub.log');
  gitEnv = {
    ...process.env,
    GIT_AUTHOR_NAME: 't',
    GIT_AUTHOR_EMAIL: 't@t',
    GIT_COMMITTER_NAME: 't',
    GIT_COMMITTER_EMAIL: 't@t',
    GIT_CONFIG_GLOBAL: '/dev/null',
    GIT_CONFIG_NOSYSTEM: '1',
  };
  for (const f of COPY) {
    fs.mkdirSync(path.dirname(path.join(repo, f)), { recursive: true });
    fs.copyFileSync(path.join(real, f), path.join(repo, f));
  }
  const conf = path.join(repo, 'apps/desktop/src-tauri/tauri.conf.json');
  fs.mkdirSync(path.dirname(conf), { recursive: true });
  fs.writeFileSync(
    conf,
    JSON.stringify({ productName: 'qrtz', version: V }, null, 2) + '\n',
  );
  fs.mkdirSync(bin);
  fs.writeFileSync(path.join(bin, 'curl'), CURL, { mode: 0o755 });
  for (const t of ['security', 'gh', 'minisign', 'codesign'])
    fs.writeFileSync(path.join(bin, t), LOGGER(t), { mode: 0o755 });
  fs.writeFileSync(log, '');
  key = path.join(tmp, 'keys/app-update.key');
  fs.mkdirSync(path.dirname(key), { recursive: true });
  fs.writeFileSync(key, KEY + '\n', { mode: 0o600 });
  target = path.join(tmp, 'target');
  const macos = path.join(target, 'release/bundle/macos');
  fs.mkdirSync(path.join(macos, 'qrtz.app/Contents/MacOS'), {
    recursive: true,
  });
  fs.writeFileSync(path.join(macos, `qrtz_${V}_aarch64.dmg`), 'dmg');
  git(['init', '-q', '-b', 'main']);
  git(['add', '-A']);
  git(['commit', '-qm', 'qrtz']);
  git(['init', '-q', '--bare', path.join(tmp, 'origin.git')], tmp);
  git(['remote', 'add', 'origin', path.join(tmp, 'origin.git')]);
  git(['push', '-q', 'origin', 'main']);
});
afterEach(() => fs.rmSync(tmp, { recursive: true, force: true }));

function run(args, env = {}) {
  const r = spawnSync(
    'bash',
    [path.join(repo, 'scripts/app/publish-local.sh'), ...args],
    {
      encoding: 'utf8',
      env: {
        ...gitEnv,
        PATH: `${bin}:${process.env.PATH}`,
        HOME: tmp,
        STUB_LOG: log,
        QRTZ_APP_UPDATE_KEY: key,
        CARGO_TARGET_DIR: target,
        GH_TOKEN: GHTOKEN,
        ...env,
      },
    },
  );
  for (const s of [KEY, GHTOKEN]) {
    expect(r.stdout).not.toContain(s);
    expect(r.stderr).not.toContain(s);
  }
  return r;
}
const plan = (r) => r.stdout.split('\n').filter((l) => l.startsWith('+ '));
const inOrder = (lines, steps) => {
  const at = steps.map((s) => lines.findIndex((l) => l.includes(s)));
  return at.every((v, i) => v >= 0 && (i === 0 || v > at[i - 1]))
    ? true
    : String(at);
};
const calls = () => fs.readFileSync(log, 'utf8');

describe('publish-local.sh --dry-run', () => {
  it("plans release.yml's steps in order, with the key only by path, and touches nothing", () => {
    const r = run(['--dry-run'], { STUB_FEED: envelope('0.2.0') });
    expect(r.status, r.stderr).toBe(0);
    const lines = plan(r);
    expect(
      inOrder(lines, [
        'COPYFILE_DISABLE=1 tar -C',
        'signer sign --app-version 0.2.1',
        'minisign -V',
        'release-notes.mjs 0.2.1',
        'feed.mjs --version 0.2.1',
        'sign-feed.sh',
        'source-archives.sh 0.2.1',
        'gh-store.sh upload-app',
        'verify-live.sh',
        'version.mjs tag --push',
      ]),
    ).toBe(true);
    const sign = lines.find((l) => l.includes('signer sign'));
    expect(sign).toContain(`TAURI_SIGNING_PRIVATE_KEY=<${key}>`);
    expect(sign).toContain('TAURI_SIGNING_PRIVATE_KEY_PASSWORD= ');
    expect(lines.find((l) => l.includes('sign-feed.sh'))).toContain(
      'TAURI_SIGNING_PRIVATE_KEY=<',
    );
    expect(lines.find((l) => l.includes('feed.mjs --version'))).toContain(
      '--expires-days 30',
    );
    expect(lines.find((l) => l.includes('gh-store.sh upload-app'))).toMatch(
      /^\+ env VERSION=0\.2\.1 /,
    );
    expect(lines.find((l) => l.includes('gh-store.sh upload-app'))).toContain(
      `qrtz_${V}_aarch64.dmg`,
    );
    expect(r.stdout).toContain('(live feed: 0.2.0)');
    expect(r.stdout).toContain('dry run, nothing done');
    // Nothing ran: no keychain, no gh, no signing, no tag, no file in the tree.
    expect(calls()).not.toMatch(/^(security|gh|minisign|codesign) /m);
    expect(git(['tag', '-l'])).toBe('');
    expect(git(['status', '--porcelain'])).toBe('');
  });

  it('reads no keychain item and names no Cloudflare credential (QRT-297)', () => {
    const r = run(['--dry-run']);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).not.toMatch(/R2_|rclone|qrtz-r2/);
    expect(calls()).not.toMatch(/^security /m);
  });

  it('--no-tag stops before the tag; --app and --dmg take explicit paths', () => {
    const app = path.join(tmp, 'elsewhere/qrtz.app');
    fs.mkdirSync(app, { recursive: true });
    const dmg = path.join(tmp, 'elsewhere/x.dmg');
    fs.writeFileSync(dmg, 'dmg');
    const r = run(['--dry-run', '--no-tag', '--app', app, '--dmg', dmg]);
    expect(r.status, r.stderr).toBe(0);
    const lines = plan(r);
    expect(lines.some((l) => l.includes('version.mjs tag'))).toBe(false);
    expect(lines.find((l) => l.includes('tar -C'))).toContain(
      path.join(tmp, 'elsewhere'),
    );
    expect(lines.find((l) => l.includes('gh-store.sh upload-app'))).toMatch(
      /x\.dmg$/,
    );
  });

  it('refuses before planning anything', () => {
    const cases = [
      [
        () => fs.writeFileSync(path.join(repo, 'stray'), 'x'),
        {},
        "tree isn't clean",
      ],
      [() => git(['tag', `app-v${V}`]), {}, `app-v${V} already exists`],
      [
        () => {
          git(['tag', `app-v${V}`]);
          git(['push', '-q', 'origin', `app-v${V}`]);
          git(['tag', '-d', `app-v${V}`]);
        },
        {},
        `app-v${V} already exists on origin`,
      ],
      [() => {}, { STUB_FEED: envelope(V) }, `live feed already has qrtz ${V}`],
      [
        () => {},
        { STUB_FEED: envelope('0.3.0') },
        'live feed already has qrtz 0.3.0',
      ],
      [() => {}, { STUB_ARCHIVE_LIVE: '1' }, 'is already up'],
      [
        () => {},
        { QRTZ_APP_UPDATE_KEY: path.join(tmp, 'nope.key') },
        'no app-update key',
      ],
      [
        () => {},
        { CARGO_TARGET_DIR: path.join(tmp, 'empty') },
        'no built qrtz.app',
      ],
      [
        () => {
          fs.writeFileSync(path.join(repo, 'later'), 'x');
          git(['add', 'later']);
          git(['commit', '-qm', 'not pushed']);
        },
        {},
        "isn't on origin's main",
      ],
    ];
    for (const [setup, env, msg] of cases) {
      const before = git(['rev-parse', 'HEAD']);
      setup();
      const r = run(['--dry-run'], env);
      expect(r.status, msg).not.toBe(0);
      expect(r.stderr).toContain(msg);
      expect(plan(r)).toEqual([]);
      // Undo for the next case.
      git(['reset', '-q', '--hard', before]);
      git(['clean', '-qfd']);
      for (const t of git(['tag', '-l']).split('\n').filter(Boolean))
        git(['tag', '-d', t]);
      spawnSync('git', ['push', '-q', 'origin', `:refs/tags/app-v${V}`], {
        cwd: repo,
        env: gitEnv,
      });
    }
    expect(run(['--bogus']).stderr).toContain('unknown argument');
    expect(run(['--refresh', '--no-tag']).stderr).toContain(
      '--refresh takes no',
    );
  });

  it('refuses a real run off macOS (the key never touches a server-side box)', () => {
    if (process.platform === 'darwin') return;
    const r = run([]);
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain("runs on Jared's Mac only");
    expect(calls()).not.toMatch(/^(security|gh) /m);
  });
});

describe('publish-local.sh --refresh --dry-run', () => {
  it('plans app-feed-refresh.yml: verify the live feed, re-sign with a new expires, upload latest.json, confirm', () => {
    const r = run(['--refresh', '--dry-run'], { STUB_FEED: envelope(V) });
    expect(r.status, r.stderr).toBe(0);
    const lines = plan(r);
    expect(
      inOrder(lines, [
        'unwrap ',
        'minisign -V',
        'feed.mjs --unwrap',
        'sign-feed.sh',
        'gh-store.sh upload-feed',
        'curl ',
      ]),
    ).toBe(true);
    expect(lines.find((l) => l.includes('feed.mjs --unwrap'))).toContain(
      '--expires-days 30',
    );
    const up = lines.find((l) => l.includes('gh-store.sh upload-feed'));
    expect(up).toContain('latest.json');
    expect(
      lines.some(
        (l) => l.includes('upload-app') || l.includes('version.mjs tag'),
      ),
    ).toBe(false);
    expect(calls()).not.toMatch(/^(security|gh|minisign) /m);
  });

  it('does nothing when no feed is live yet', () => {
    const r = run(['--refresh', '--dry-run']);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain('nothing to refresh');
    expect(plan(r)).toEqual([]);
  });
});

describe('gh-store.sh upload-app (what publish-local.sh runs for the upload)', () => {
  it('uploads the immutable versioned files first and latest.json last', () => {
    const dir = path.join(tmp, 'work');
    fs.mkdirSync(dir);
    const tgz = path.join(dir, `qrtz_${V}_aarch64.app.tar.gz`);
    fs.writeFileSync(tgz, 'tgz');
    fs.writeFileSync(`${tgz}.sig`, 'sig');
    fs.copyFileSync(envelope(V), path.join(dir, 'latest.json'));
    const dmg = path.join(tmp, 'q.dmg');
    fs.writeFileSync(dmg, 'dmg');
    const srcs = [
      `qrtz_${V}_source.tar.gz`,
      `qrtz_${V}_fluidaudio_source.tar.gz`,
      `qrtz_${V}_build-scripts.tar.gz`,
      `qrtz_${V}_source.sha256`,
    ];
    const up = () =>
      spawnSync(
        'bash',
        [
          path.join(repo, 'scripts/release/gh-store.sh'),
          'upload-app',
          dir,
          dmg,
        ],
        {
          encoding: 'utf8',
          env: {
            PATH: `${bin}:${process.env.PATH}`,
            STUB_LOG: log,
            VERSION: V,
            GH_TOKEN: GHTOKEN,
          },
        },
      );
    // QRT-275: no release goes up without its source.
    const missing = up();
    expect(missing.status).not.toBe(0);
    expect(missing.stderr).toContain('source.tar.gz');
    for (const f of srcs) fs.writeFileSync(path.join(dir, f), f);
    const r = up();
    expect(r.status, r.stderr).toBe(0);
    const ups = calls()
      .split('\n')
      .filter((l) => l.startsWith('gh release upload'));
    expect(ups).toHaveLength(9);
    // The versioned files (with their source, QRT-275) are never replaced; only the stable dmg and the feed are, the feed last.
    for (const u of ups.slice(0, 7)) expect(u).not.toContain('--clobber');
    expect(ups[0]).toMatch(
      /\/v1\.app\.0\.2\.1\.qrtz_0\.2\.1_aarch64\.app\.tar\.gz$/,
    );
    for (const f of srcs)
      expect(ups.slice(3, 7).some((u) => u.endsWith(`v1.app.${V}.${f}`))).toBe(
        true,
      );
    expect(ups[7]).toMatch(/\/v1\.app\.qrtz-latest\.dmg --clobber$/);
    expect(ups[8]).toMatch(/\/v1\.app\.latest\.json --clobber$/);
    expect(calls()).not.toContain(GHTOKEN);
  });
});

// Audit P0-1: the release wrapper's plan, checked on any OS through its dry run (QRTZ_SIGN_DRY_RUN=1).
import { describe, expect, it } from 'vitest';
import { spawnSync } from 'node:child_process';
import path from 'node:path';

const script = path.join(import.meta.dirname, 'sign-notarize.sh');
const ID = 'Developer ID Application: Jared Grimes (ABCDE12345)';

function run(env, args = []) {
  return spawnSync('bash', [script, ...args], {
    encoding: 'utf8',
    env: { PATH: process.env.PATH, QRTZ_SIGN_DRY_RUN: '1', ...env },
  });
}
const plan = (r) => r.stdout.split('\n').filter((l) => l.startsWith('+ '));

describe('sign-notarize.sh', () => {
  it('builds signed but not notarized, hands nesting to sign-mac.sh, then checks and hashes', () => {
    const r = run({ QRTZ_SIGN_ID: ID, QRTZ_TEAM_ID: 'ABCDE12345' });
    expect(r.status, r.stderr).toBe(0);
    const lines = plan(r);
    const at = (s) => lines.findIndex((l) => l.includes(s));
    const make = lines[at('run --cwd')];
    expect(make).toMatch(/ make$/);
    for (const v of [
      'APPLE_ID',
      'APPLE_PASSWORD',
      'APPLE_API_KEY',
      'APPLE_API_ISSUER',
    ])
      expect(make).toContain(`-u ${v} `);
    expect(make).toContain(
      'APPLE_SIGNING_IDENTITY=Developer\\ ID\\ Application:',
    );
    const sign = lines[at('sign-mac.sh')];
    expect(sign).toContain('QRTZ_NOTARY_PROFILE=qrtz-notary');
    expect(sign).not.toContain('--no-notarize');
    const order = [
      'run --cwd',
      'sign-mac.sh',
      'codesign -dv',
      'stapler validate',
      'spctl --assess --type open',
      'shasum -a 256',
    ].map(at);
    expect(
      order.every((v, i) => v >= 0 && (i === 0 || v > order[i - 1])),
      String(order),
    ).toBe(true);
  });

  it('signs an existing build, takes the notary profile from the env and can skip notarizing', () => {
    const env = {
      QRTZ_SIGN_ID: ID,
      QRTZ_TEAM_ID: 'ABCDE12345',
      QRTZ_NOTARY_PROFILE: 'mine',
    };
    const r = run(env, ['--app', '/tmp/x/qrtz.app', '--no-notarize']);
    expect(r.status, r.stderr).toBe(0);
    const lines = plan(r);
    expect(lines.some((l) => l.includes('run --cwd'))).toBe(false);
    const sign = lines.find((l) => l.includes('sign-mac.sh'));
    expect(sign).toContain('QRTZ_NOTARY_PROFILE=mine');
    expect(sign).toMatch(/\/tmp\/x\/qrtz\.app --no-notarize$/);
    expect(r.stdout).not.toContain('stapler');
  });

  it("defaults to Jared's Developer ID identity, team MR26CCZ496 and the qrtz-notary profile", () => {
    const r = run({});
    expect(r.status, r.stderr).toBe(0);
    const lines = plan(r);
    const make = lines.find((l) => l.includes('run --cwd'));
    expect(make).toContain(
      'APPLE_SIGNING_IDENTITY=Developer\\ ID\\ Application:\\ Jared\\ Grimes\\ \\(MR26CCZ496\\)',
    );
    const sign = lines.find((l) => l.includes('sign-mac.sh'));
    expect(sign).toContain('QRTZ_NOTARY_PROFILE=qrtz-notary');
    expect(sign).toContain('(MR26CCZ496\\)');
  });

  it('APPLE_SIGNING_IDENTITY overrides the default identity when QRTZ_SIGN_ID is unset', () => {
    const r = run({ APPLE_SIGNING_IDENTITY: ID, QRTZ_TEAM_ID: 'ABCDE12345' });
    expect(r.status, r.stderr).toBe(0);
    const make = plan(r).find((l) => l.includes('run --cwd'));
    expect(make).toContain('\\(ABCDE12345\\)');
    expect(make).not.toContain('MR26CCZ496');
  });

  it('package.json release:mac runs this script', async () => {
    const { readFileSync } = await import('node:fs');
    const pkg = JSON.parse(
      readFileSync(
        path.join(import.meta.dirname, '../../package.json'),
        'utf8',
      ),
    );
    expect(pkg.scripts['release:mac']).toBe(
      'bash scripts/release/sign-notarize.sh',
    );
  });

  it('refuses missing or mismatched credentials before doing anything', () => {
    for (const [env, msg] of [
      [{ QRTZ_SIGN_ID: ID }, "doesn't name team MR26CCZ496"],
      [{ QRTZ_SIGN_ID: ID, QRTZ_TEAM_ID: 'ZZZZZ99999' }, "doesn't name team"],
      [{ QRTZ_SIGN_ID: ID, QRTZ_TEAM_ID: 'short' }, '10 characters'],
      [
        {
          QRTZ_SIGN_ID: 'Apple Development: Jared (ABCDE12345)',
          QRTZ_TEAM_ID: 'ABCDE12345',
        },
        'Developer ID Application',
      ],
      [
        {
          QRTZ_SIGN_ID: 'Developer ID Application: x (0000000000)',
          QRTZ_TEAM_ID: '0000000000',
        },
        'placeholder',
      ],
    ]) {
      const r = run(env);
      expect(r.status).not.toBe(0);
      expect(r.stderr).toContain(msg);
      expect(r.stdout).toBe('');
    }
  });
});

describe('sign-notarize.sh QRTZ_SPEECH_* (all or none)', () => {
  const base = { QRTZ_SIGN_ID: ID, QRTZ_TEAM_ID: 'ABCDE12345' };
  const all = {
    QRTZ_SPEECH_URL: 'https://example.invalid/s',
    QRTZ_SPEECH_KEY_ID: 'k1',
    QRTZ_SPEECH_KEY: 'secret-key-value',
    QRTZ_SPEECH_ROOT_KEY_ID: 'r1',
    QRTZ_SPEECH_ROOT_KEY: 'secret-root-value',
  };
  it('none set: builds without speech', () => {
    const r = run(base);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stderr).toContain('without a speech module');
    expect(plan(r).some((l) => l.includes('run --cwd'))).toBe(true);
  });
  it('all set: builds with speech and never prints a value', () => {
    const r = run({ ...base, ...all });
    expect(r.status, r.stderr).toBe(0);
    expect(r.stderr).toContain('all 5 QRTZ_SPEECH_* set');
    expect(r.stdout + r.stderr).not.toMatch(/secret-(key|root)-value/);
  });
  it('only some set: stops before running anything, names only', () => {
    for (const drop of Object.keys(all)) {
      const env = { ...base, ...all };
      delete env[drop];
      const r = run(env);
      expect(r.status).not.toBe(0);
      expect(r.stderr).toContain(`missing: ${drop}`);
      expect(plan(r)).toEqual([]);
      expect(r.stdout + r.stderr).not.toMatch(/secret-(key|root)-value/);
    }
    const r = run({ ...base, QRTZ_SPEECH_URL: 'https://example.invalid/s' });
    expect(r.status).not.toBe(0);
    expect(plan(r)).toEqual([]);
  });
});

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

  it('refuses missing or mismatched credentials before doing anything', () => {
    for (const [env, msg] of [
      [{}, 'QRTZ_SIGN_ID'],
      [{ QRTZ_SIGN_ID: ID }, 'QRTZ_TEAM_ID'],
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

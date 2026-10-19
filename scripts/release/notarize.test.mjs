// notarize.sh: only Apple's "Accepted" passes, whatever notarytool's exit code says (Bobs: `submit --wait` can exit
// 0 on a rejected upload). A stub xcrun on PATH plays notarytool.
import { describe, expect, it } from 'vitest';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

const script = path.join(import.meta.dirname, 'notarize.sh');
const ID = '2efe2717-52ef-43a5-96dc-0797e4ca1041';

function run(stdout, code = 0, stderr = '') {
  const dir = mkdtempSync(path.join(tmpdir(), 'notary-'));
  writeFileSync(path.join(dir, 'out'), stdout);
  writeFileSync(path.join(dir, 'err'), stderr);
  writeFileSync(
    path.join(dir, 'xcrun'),
    `#!/usr/bin/env bash\necho "$@" > "${dir}/args"\ncat "${dir}/out"\ncat "${dir}/err" >&2\nexit ${code}\n`,
  );
  chmodSync(path.join(dir, 'xcrun'), 0o755);
  const r = spawnSync('bash', [script, '/tmp/qrtz.zip', 'qrtz-notary'], {
    encoding: 'utf8',
    env: { PATH: `${dir}:${process.env.PATH}` },
  });
  return { ...r, args: readFileSync(path.join(dir, 'args'), 'utf8').trim() };
}
const json = (status) =>
  JSON.stringify({ id: ID, status, message: `Processing complete` }) + '\n';

describe('notarize.sh', () => {
  it('passes on Accepted, asking for JSON from a waiting submit', () => {
    const r = run(json('Accepted'));
    expect(r.status, r.stderr).toBe(0);
    expect(r.args).toBe(
      'notarytool submit /tmp/qrtz.zip --keychain-profile qrtz-notary --wait --output-format json',
    );
    expect(r.stdout).toContain(ID);
  });

  it('fails on Invalid even when notarytool exits 0, with the id and the log command', () => {
    const r = run(json('Invalid'));
    expect(r.status).toBe(1);
    expect(r.stderr).toContain('status Invalid');
    expect(r.stderr).toContain(`submission id: ${ID}`);
    expect(r.stderr).toContain(
      `xcrun notarytool log ${ID} --keychain-profile qrtz-notary`,
    );
  });

  it('fails on a wait that ended In Progress, or a submit with no JSON at all', () => {
    expect(run(json('In Progress')).status).toBe(1);
    const r = run(
      '',
      69,
      'Error: HTTP status code: 401. Unable to authenticate.',
    );
    expect(r.status).toBe(1);
    expect(r.stderr).toContain('status unknown, notarytool exit 69');
    expect(r.stderr).toContain('no submission id');
    expect(r.stderr).toContain('Unable to authenticate');
  });
});

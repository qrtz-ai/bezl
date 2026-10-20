// publish.sh --out-only is a dry run: it builds in a mktemp scratch copy and never writes to $QRTZ_PUBLISH_OUT,
// which must keep mirroring what the index release serves.
import { execFileSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';

const script = path.join(import.meta.dirname, 'publish.sh');
const tmps = [];
afterEach(() => {
  for (const d of tmps.splice(0))
    fs.rmSync(d, { recursive: true, force: true });
});

// Every file under dir (relative path, mode, sha256), so "byte-identical" covers additions and deletions too.
const snapshot = (dir) => {
  const out = [];
  const walk = (d) => {
    for (const e of fs
      .readdirSync(d, { withFileTypes: true })
      .sort((a, b) => a.name.localeCompare(b.name))) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else {
        const st = fs.statSync(p);
        out.push(
          `${path.relative(dir, p)} ${st.mode} ${st.mtimeMs} ${crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex')}`,
        );
      }
    }
  };
  walk(dir);
  return out;
};

const setup = () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-publish-test-'));
  tmps.push(root);
  const keys = path.join(root, 'keys');
  const out = path.join(root, 'out');
  fs.mkdirSync(keys);
  fs.writeFileSync(path.join(keys, 'timestamp.key'), 'not a key');
  fs.writeFileSync(path.join(keys, 'index.key'), 'not a key');
  fs.mkdirSync(path.join(out, 'v1/root'), { recursive: true });
  fs.writeFileSync(path.join(out, 'v1/root/1.root.json'), '{"root":1}\n');
  fs.writeFileSync(path.join(out, 'v1/timestamp.json'), '{"live":true}\n');
  const pin = crypto.createHash('sha256').update('{"root":1}\n').digest('hex');
  // A stand-in qrtz-publish that writes into whatever --out it is given, like the real timestamp/build/verify do.
  const bin = path.join(root, 'qrtz-publish');
  fs.writeFileSync(
    bin,
    `#!/usr/bin/env bash
set -eu
cmd="$1"; shift; o=""
while [[ $# -gt 0 ]]; do [[ "$1" == --out ]] && o="$2"; shift; done
echo "$cmd $o" >>"${root}/calls"
case "$cmd" in
  timestamp) echo '{"resigned":true}' >"$o/v1/timestamp.json"; echo sig >"$o/v1/timestamp.json.sig" ;;
  verify) touch "$o/v1/verified" ;;
  upload-list) printf 'v1/timestamp.json.sig\\t4\\tpublic, max-age=300\\nv1/timestamp.json\\t18\\tpublic, max-age=300\\n' ;;
esac
`,
  );
  fs.chmodSync(bin, 0o755);
  const env = {
    ...process.env,
    QRTZ_KEYS: keys,
    QRTZ_PUBLISH_OUT: out,
    QRTZ_ROOT1_SHA256: pin,
    QRTZ_PUBLISH_BIN: bin,
    QRTZ_PRIMARY_URL: 'http://127.0.0.1:9',
    TMPDIR: root,
  };
  return { root, out, env };
};

describe('scripts/modules/publish.sh --out-only', () => {
  it('leaves the out dir byte-identical, works in a scratch copy and deletes it on exit', () => {
    const { root, out, env } = setup();
    const before = snapshot(out);
    const kept = path.join(root, 'kept.tsv');
    const log = execFileSync('bash', [script, '--timestamp', '--out-only'], {
      env: { ...env, QRTZ_KEEP_UPLOAD_LIST: kept },
      encoding: 'utf8',
    });
    expect(snapshot(out)).toEqual(before);
    expect(fs.existsSync(`${out}.upload.tsv`)).toBe(false);
    const calls = fs
      .readFileSync(path.join(root, 'calls'), 'utf8')
      .trim()
      .split('\n');
    const scratch = calls[0].split(' ')[1];
    expect(scratch).not.toBe(out);
    expect(calls).toEqual([
      `timestamp ${scratch}`,
      `verify ${scratch}`,
      `upload-list ${scratch}`,
    ]);
    expect(log).toContain(`scratch copy ${scratch}`);
    // The log said what would upload; then the scratch copy and its upload list are gone.
    expect(log).toContain('publish:   v1/timestamp.json\t18');
    expect(fs.existsSync(scratch)).toBe(false);
    expect(fs.existsSync(`${scratch}.upload.tsv`)).toBe(false);
    expect(
      fs.readdirSync(root).filter((f) => f.startsWith('qrtz-publish-dry')),
    ).toEqual([]);
    expect(fs.readFileSync(kept, 'utf8')).toContain('v1/timestamp.json\t18');
    expect(log).toContain(`upload list kept at ${kept}`);
  });
  it('keeps no upload list unless asked, and cleans up a failed dry run too', () => {
    const { root, env } = setup();
    execFileSync('bash', [script, '--timestamp', '--out-only'], {
      env,
      stdio: 'pipe',
    });
    expect(
      fs.readdirSync(root).filter((f) => f.startsWith('qrtz-publish-dry')),
    ).toEqual([]);
    expect(() =>
      execFileSync('bash', [script, '--timestamp', '--out-only'], {
        env: { ...env, QRTZ_ROOT1_SHA256: '0'.repeat(64) },
        stdio: 'pipe',
      }),
    ).toThrow();
    expect(
      fs.readdirSync(root).filter((f) => f.startsWith('qrtz-publish-dry')),
    ).toEqual([]);
  });
  it('refuses a dry run with no out dir to copy', () => {
    const { out, env } = setup();
    fs.rmSync(path.join(out, 'v1'), { recursive: true });
    expect(() =>
      execFileSync('bash', [script, '--timestamp', '--out-only'], {
        env,
        stdio: 'pipe',
      }),
    ).toThrow();
  });
});

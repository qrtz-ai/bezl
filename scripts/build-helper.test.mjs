// build-helper.mjs passes SWIFT_BUILD_FLAGS to every swift build / swift test (no PATH shim for --disable-keychain).
// Checked on any OS through `--test` with a fake `swift` on PATH that logs its arguments.
import { describe, expect, it } from 'vitest';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const script = path.join(import.meta.dirname, 'build-helper.mjs');

function run(env) {
  const bin = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-fake-swift-'));
  const log = path.join(bin, 'calls.log');
  fs.writeFileSync(
    path.join(bin, 'swift'),
    `#!/bin/sh\necho "$*" >> "${log}"\n[ "$1" = test ] && echo "Executed 3 tests, with 0 failures"\nexit 0\n`,
    { mode: 0o755 },
  );
  const r = spawnSync(process.execPath, [script, '--test'], {
    encoding: 'utf8',
    env: {
      PATH: `${bin}${path.delimiter}${process.env.PATH}`,
      QRTZ_REQUIRE_SWIFT: '1',
      ...env,
    },
  });
  const calls = fs
    .readFileSync(log, 'utf8')
    .split('\n')
    .filter((l) => l && l !== '--version');
  return { r, calls };
}

describe('build-helper.mjs SWIFT_BUILD_FLAGS', () => {
  it('appends the split flags to swift build and swift test', () => {
    const { r, calls } = run({
      SWIFT_BUILD_FLAGS: '  --disable-keychain   -Xswiftc -warnings-as-errors ',
    });
    expect(r.status, r.stderr).toBe(0);
    expect(calls).toEqual([
      'build --disable-keychain -Xswiftc -warnings-as-errors',
      'test --disable-swift-testing --disable-keychain -Xswiftc -warnings-as-errors',
    ]);
  });

  it('passes nothing extra when unset', () => {
    const { r, calls } = run({});
    expect(r.status, r.stderr).toBe(0);
    expect(calls).toEqual(['build', 'test --disable-swift-testing']);
  });
});

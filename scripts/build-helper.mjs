// Builds/tests native/qrtz-helper with SwiftPM.
//   bun run build:helper  → macOS: release arm64 binary that the Tauri bundle ships in Contents/Resources. Elsewhere: skipped
//                           (the app uses the TS mock helper).
//   bun run test:helper   → `swift build && swift test` anywhere a Swift toolchain is on PATH (Linux builds the
//                           Foundation-only core + stub executable; macOS builds everything). Skipped if no toolchain.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const dir = path.resolve(import.meta.dirname, '../native/qrtz-helper');
// Toolchain: PATH, then QRTZ_SWIFT_TOOLCHAIN (a .../usr/bin dir), then the LOOP.md location on Linux.
const candidates = [
  process.env.QRTZ_SWIFT_TOOLCHAIN,
  '/tmp/swift-6.1.2-RELEASE-ubuntu24.04/usr/bin',
].filter(Boolean);
const ok = (bin) =>
  spawnSync(bin, ['--version'], { stdio: 'ignore' }).status === 0;
let swift = ok('swift') ? 'swift' : undefined;
for (const c of candidates) {
  const bin = path.join(c, 'swift');
  if (!swift && fs.existsSync(bin) && ok(bin)) swift = bin;
}
const run = (args) =>
  spawnSync(swift, args, { cwd: dir, stdio: 'inherit' }).status ?? 1;

if (process.argv.includes('--test')) {
  // CI-like runs must not pass silently without Swift.
  const required =
    !!process.env.CI ||
    process.env.QRTZ_REQUIRE_SWIFT === '1' ||
    process.platform === 'darwin';
  if (!swift) {
    const msg =
      '[test:helper] no Swift toolchain (PATH, QRTZ_SWIFT_TOOLCHAIN, /tmp/swift-6.1.2-RELEASE-ubuntu24.04)';
    if (required) {
      console.error(`${msg}: FAILING (CI / QRTZ_REQUIRE_SWIFT=1 / macOS)`);
      process.exit(1);
    }
    console.warn(
      `${msg}: SKIPPED. Set QRTZ_REQUIRE_SWIFT=1 to make this fatal.`,
    );
    process.exit(0);
  }
  if (run(['build'])) process.exit(1);
  // XCTest only: Swift 6 also runs an (empty) swift-testing pass whose "0 tests passed" line was easy to
  // misread as "nothing ran". We count XCTest results ourselves and fail on zero.
  const r = spawnSync(swift, ['test', '--disable-swift-testing'], {
    cwd: dir,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  process.stdout.write(r.stdout ?? '');
  process.stderr.write(r.stderr ?? '');
  const out = `${r.stdout}\n${r.stderr}`;
  const m = [
    ...out.matchAll(/Executed (\d+) tests?, with (\d+) failures?/g),
  ].at(-1);
  const executed = m ? Number(m[1]) : 0;
  const failures = m ? Number(m[2]) : 0;
  if (r.status !== 0 || failures > 0) {
    console.error(
      `[test:helper] FAILED: ${failures} of ${executed} Swift tests failed`,
    );
    process.exit(1);
  }
  if (executed === 0) {
    console.error(
      '[test:helper] FAILED: 0 Swift tests ran (expected the XCTest suite)',
    );
    process.exit(1);
  }
  console.log(`[test:helper] ${executed} Swift tests passed`);
  process.exit(0);
}
if (process.platform !== 'darwin') {
  console.log(
    '[build:helper] not macOS: skipping (app uses the TS mock helper)',
  );
  process.exit(0);
}
process.exit(run(['build', '-c', 'release', '--arch', 'arm64']));

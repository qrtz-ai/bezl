// Builds/tests native/qrtz-helper with SwiftPM.
//   bun run build:helper  → macOS: the `qrtz` command, the bundle's one externalBin (ADR 0169: no module code is
//                           bundled; scripts/modules/build-packages.sh builds the module packages). Elsewhere: skipped.
//   bun run test:helper   → `swift build && swift test` anywhere a Swift toolchain is on PATH (Linux builds the
//                           Foundation-only core + stub executable; macOS builds everything). Skipped if no toolchain.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { install, isStub, syncDevCopy } from './sidecars.mjs';

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
    // "Executed 6 tests, with 1 test skipped and 0 failures": XCTSkip adds the skipped clause.
    ...out.matchAll(
      /Executed (\d+) tests?, with (?:\d+ tests? skipped and )?(\d+) failures?/g,
    ),
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
// ADR 0169: qrtz ships empty. No module code goes into the bundle any more: each module's executable (qrtz-echo,
// qrtz-nano, qrtz-ctrl, qrtz-snap) ships signed in its own package (scripts/modules/build-packages.sh) and is
// installed from the marketplace. A stale qrtz-helper or qrtz-snap sidecar left from an older build is removed so it
// can't be mistaken for a bundled one.
const binDir = path.resolve(
  import.meta.dirname,
  '../apps/desktop/src-tauri/binaries',
);
fs.mkdirSync(binDir, { recursive: true });
const repo = path.resolve(import.meta.dirname, '..');
const targetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(repo, process.env.CARGO_TARGET_DIR)
  : path.join(repo, 'target');
for (const old of ['qrtz-helper', 'qrtz-snap']) {
  fs.rmSync(path.join(binDir, `${old}-aarch64-apple-darwin`), { force: true });
  fs.rmSync(path.join(targetDir, 'debug', old), { force: true });
}

// ADR 0162: the `qrtz` command ships as the one externalBin (qrtz.app/Contents/MacOS/qrtz; the app itself is
// Contents/MacOS/qrtz-app). Built here so `bun run make` and the release workflow (which runs build:helper) bundle the
// real one; a debug `cargo build` without it gets a stub from src-tauri/build.rs.
const cargo = spawnSync(
  'cargo',
  [
    'build',
    '--release',
    '-p',
    'qrtz-app',
    '--features',
    'runtime',
    '--bin',
    'qrtz',
  ],
  { cwd: repo, stdio: 'inherit' },
);
if (cargo.status !== 0) process.exit(1);
const cliDest = path.join(binDir, 'qrtz-aarch64-apple-darwin');
install(path.join(targetDir, 'release', 'qrtz'), cliDest);
// Hardened runtime and a secure timestamp on the nested command (no entitlements, it needs none). The bundler signs
// it again inside the app, and scripts/release/sign-mac.sh fixes that up; signing here means a local `bun run make`
// never has it without the runtime flag.
const identity = process.env.APPLE_SIGNING_IDENTITY;
if (identity) {
  const sign = spawnSync(
    'codesign',
    [
      '--force',
      '--options',
      'runtime',
      '--timestamp',
      '--identifier',
      'ai.qrtz.cli',
      '--sign',
      identity,
      cliDest,
    ],
    { stdio: 'inherit' },
  );
  if (sign.status !== 0) process.exit(1);
}
console.log(`[build:helper] ${path.relative(process.cwd(), cliDest)}`);
// The copies `tauri dev` runs (target/debug/<name>), in case cargo doesn't rerun build.rs before it starts them.
for (const [file, name] of [[cliDest, 'qrtz']]) {
  if (isStub(file)) {
    console.error(`[build:helper] ${file} is still the dev stub`);
    process.exit(1);
  }
  // target/debug/qrtz may be cargo's own debug build of the command: only a stub there is replaced.
  const devCopy = path.join(targetDir, 'debug', name);
  if (name === 'qrtz' && fs.existsSync(devCopy) && !isStub(devCopy)) continue;
  if (syncDevCopy(file, name, targetDir))
    console.log(`[build:helper] synced ${path.join(targetDir, 'debug', name)}`);
}
process.exit(0);

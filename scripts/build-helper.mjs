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
if (run(['build', '-c', 'release', '--arch', 'arm64'])) process.exit(1);
// Tauri's externalBin (tauri.macos.conf.json) wants the sidecar at src-tauri/binaries/qrtz-helper-<target triple>;
// `cargo build` / `tauri dev` fail in the build script without it.
// The output dir depends on the SwiftPM build system (.build/arm64-apple-macosx/release with the native one,
// .build/out/Products/Release with swiftbuild on newer Xcode), so ask SwiftPM.
const binPath = spawnSync(
  swift,
  ['build', '-c', 'release', '--arch', 'arm64', '--show-bin-path'],
  { cwd: dir, encoding: 'utf8' },
);
if (binPath.status !== 0) process.exit(1);
const built = path.join(binPath.stdout.trim(), 'qrtz-helper');
const binDir = path.resolve(
  import.meta.dirname,
  '../apps/desktop/src-tauri/binaries',
);
fs.mkdirSync(binDir, { recursive: true });
const dest = path.join(binDir, 'qrtz-helper-aarch64-apple-darwin');
fs.copyFileSync(built, dest);
fs.chmodSync(dest, 0o755);
console.log(`[build:helper] ${path.relative(process.cwd(), dest)}`);
// ADR 0159: qrtz-snap ships next to the helper (Contents/MacOS/qrtz-snap, the third externalBin); the same SwiftPM
// build made it. Its signature carries no entitlement (ai.qrtz.snap.entitlements is empty) and the identifier its
// lock entry pins. tauri build re-signs every externalBin with the app's entitlements, so
// scripts/release/sign-mac.sh signs it again after the bundle (docs/modules/confinement-map.json bundledCode).
const snapDest = path.join(binDir, 'qrtz-snap-aarch64-apple-darwin');
fs.copyFileSync(path.join(binPath.stdout.trim(), 'qrtz-snap'), snapDest);
fs.chmodSync(snapDest, 0o755);
console.log(`[build:helper] ${path.relative(process.cwd(), snapDest)}`);

// ADR 0162: the `qrtz` command ships as the second externalBin (qrtz.app/Contents/MacOS/qrtz; the app itself is
// Contents/MacOS/qrtz-app). Built here so `bun run make` and the release workflow (which runs build:helper) bundle the
// real one; a debug `cargo build` without it gets a stub from src-tauri/build.rs.
const repo = path.resolve(import.meta.dirname, '..');
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
const targetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(repo, process.env.CARGO_TARGET_DIR)
  : path.join(repo, 'target');
const cliDest = path.join(binDir, 'qrtz-aarch64-apple-darwin');
fs.copyFileSync(path.join(targetDir, 'release', 'qrtz'), cliDest);
fs.chmodSync(cliDest, 0o755);
// Hardened runtime and a secure timestamp on the nested command (no entitlements, it needs none) and on qrtz-snap.
// The bundler signs both again inside the app, and scripts/release/sign-mac.sh fixes that up; signing here means a
// local `bun run make` never has them without the runtime flag.
const identity = process.env.APPLE_SIGNING_IDENTITY;
if (identity) {
  const signs = [
    [cliDest, ['--identifier', 'ai.qrtz.cli']],
    [
      snapDest,
      [
        '--identifier',
        'ai.qrtz.snap',
        '--entitlements',
        path.join(dir, 'entitlements/ai.qrtz.snap.entitlements'),
      ],
    ],
  ];
  for (const [file, extra] of signs) {
    const sign = spawnSync(
      'codesign',
      [
        '--force',
        '--options',
        'runtime',
        '--timestamp',
        ...extra,
        '--sign',
        identity,
        file,
      ],
      { stdio: 'inherit' },
    );
    if (sign.status !== 0) process.exit(1);
  }
}
console.log(`[build:helper] ${path.relative(process.cwd(), cliDest)}`);
process.exit(0);

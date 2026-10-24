// Builds/tests native/qrtz-helper with SwiftPM.
//   bun run build:helper  → macOS: the bundle's externalBins: the `qrtz` command and the four module helpers
//                           (ADR 0174, ADR 0237: qrtz-echo, qrtz-nano, qrtz-ctrl, qrtz-snap). Elsewhere: skipped.
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
// SWIFT_BUILD_FLAGS (e.g. `--disable-keychain`, which macOS needs so SwiftPM never stalls on a keychain prompt) is
// split on whitespace and appended to every `swift build` / `swift test`, as scripts/modules/build-packages.sh does.
const extraFlags = (process.env.SWIFT_BUILD_FLAGS ?? '')
  .split(/\s+/)
  .filter(Boolean);
const withFlags = (args) => [...args, ...extraFlags];
const run = (args) =>
  spawnSync(swift, withFlags(args), { cwd: dir, stdio: 'inherit' }).status ?? 1;

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
  const r = spawnSync(swift, withFlags(['test', '--disable-swift-testing']), {
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
// ADR 0174, ADR 0237: every first-party module's helper is bundled: qrtz-echo, qrtz-nano, qrtz-ctrl and qrtz-snap in
// Contents/MacOS, each signed ai.qrtz.<id> with exactly its own entitlements (module.json; pinned by
// scripts/release/verify-bundle.sh). A stale qrtz-helper sidecar left from an older build is removed so it can't be
// mistaken for a bundled one.
const binDir = path.resolve(
  import.meta.dirname,
  '../apps/desktop/src-tauri/binaries',
);
fs.mkdirSync(binDir, { recursive: true });
const repo = path.resolve(import.meta.dirname, '..');
const targetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(repo, process.env.CARGO_TARGET_DIR)
  : path.join(repo, 'target');
for (const old of ['qrtz-helper']) {
  fs.rmSync(path.join(binDir, `${old}-aarch64-apple-darwin`), { force: true });
  fs.rmSync(path.join(targetDir, 'debug', old), { force: true });
}

// The bundled helpers: SwiftPM products, built release arm64. tauri build re-signs them with the app's entitlements,
// so scripts/release/sign-mac.sh signs them again (confinement-map bundledCode).
const BUNDLED_HELPERS = ['echo', 'nano', 'ctrl', 'snap'];
const swiftDir = path.join(repo, 'native/qrtz-helper');
if (!swift) {
  console.error(
    '[build:helper] no Swift toolchain: the module helpers are bundled (ADR 0174, ADR 0237)',
  );
  process.exit(1);
}
for (const id of BUNDLED_HELPERS) {
  const b = spawnSync(
    swift,
    withFlags([
      'build',
      '-c',
      'release',
      '--arch',
      'arm64',
      '--product',
      `qrtz-${id}`,
    ]),
    { cwd: swiftDir, stdio: 'inherit' },
  );
  if (b.status !== 0) process.exit(1);
}
const helperBin = spawnSync(
  swift,
  withFlags(['build', '-c', 'release', '--arch', 'arm64', '--show-bin-path']),
  { cwd: swiftDir, encoding: 'utf8' },
);
if (helperBin.status !== 0) process.exit(1);
const helperDests = BUNDLED_HELPERS.map((id) => {
  const dest = path.join(binDir, `qrtz-${id}-aarch64-apple-darwin`);
  install(path.join(helperBin.stdout.trim(), `qrtz-${id}`), dest);
  console.log(`[build:helper] ${path.relative(process.cwd(), dest)}`);
  return [id, dest];
});
// Entitlements per helper, from its module.json (none: sign without --entitlements).
const helperEntitlements = (id) => {
  const m = JSON.parse(
    fs.readFileSync(path.join(repo, 'modules', id, 'module.json'), 'utf8'),
  );
  return (m.entitlements ?? []).length
    ? [
        '--entitlements',
        path.join(swiftDir, `entitlements/ai.qrtz.${id}.entitlements`),
      ]
    : [];
};

// ADR 0162: the `qrtz` command ships as an externalBin (qrtz.app/Contents/MacOS/qrtz; the app itself is
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
  const signs = [
    [cliDest, ['--identifier', 'ai.qrtz.cli']],
    ...helperDests.map(([id, dest]) => [
      dest,
      ['--identifier', `ai.qrtz.${id}`, ...helperEntitlements(id)],
    ]),
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
// The copies `tauri dev` runs (target/debug/<name>), in case cargo doesn't rerun build.rs before it starts them.
for (const [file, name] of [
  ...helperDests.map(([id, dest]) => [dest, `qrtz-${id}`]),
  [cliDest, 'qrtz'],
]) {
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

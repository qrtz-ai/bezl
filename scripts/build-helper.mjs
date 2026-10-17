// Builds/tests native/qrtz-helper with SwiftPM.
//   npm run build:helper  → macOS: release arm64 binary that Forge bundles into Contents/Resources. Elsewhere: skipped
//                           (the app uses the TS mock helper).
//   npm run test:helper   → `swift build && swift test` anywhere a Swift toolchain is on PATH (Linux builds the
//                           Foundation-only core + stub executable; macOS builds everything). Skipped if no toolchain.
import { spawnSync } from 'node:child_process';
import path from 'node:path';

const dir = path.resolve(import.meta.dirname, '../native/qrtz-helper');
const run = (args) =>
  spawnSync('swift', args, { cwd: dir, stdio: 'inherit' }).status ?? 1;
const hasSwift =
  spawnSync('swift', ['--version'], { stdio: 'ignore' }).status === 0;

if (process.argv.includes('--test')) {
  if (!hasSwift) {
    console.log('[test:helper] no Swift toolchain on PATH: skipping');
    process.exit(0);
  }
  process.exit(run(['build']) || run(['test']));
}
if (process.platform !== 'darwin') {
  console.log(
    '[build:helper] not macOS: skipping (app uses the TS mock helper)',
  );
  process.exit(0);
}
process.exit(run(['build', '-c', 'release', '--arch', 'arm64']));

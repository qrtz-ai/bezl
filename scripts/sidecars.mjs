// How build-helper.mjs puts the Tauri externalBin (the `qrtz` command; ADR 0169: no module code) in place.
//
// Cargo reruns src-tauri/build.rs (and so tauri-build's copy of each externalBin into target/<profile>/) only when a
// `rerun-if-changed` file's mtime is newer than its last run. macOS copies (fs.copyFileSync, cp -p, clonefile) keep
// the source's mtime, which is when SwiftPM linked it, often before the last build-script run: a dev stub that
// build.rs wrote, or an old helper, then stayed in target/debug and `tauri dev` ran it. So a changed binary is
// written fresh (new mtime, new inode via rename, so a running copy is never patched in place), an unchanged one is
// left alone (no needless rebuild of the desktop crate), and the dev copy in target/debug is synced the same way.
import fs from 'node:fs';
import path from 'node:path';

/** The marker src-tauri/build.rs puts in a dev stub. */
export const STUB_MARK = 'qrtz: dev stub';

export const isStub = (file) => {
  try {
    return fs.readFileSync(file).includes(STUB_MARK);
  } catch {
    return false;
  }
};

/** Writes `src` to `dest` (0755) if their bytes differ. True if it wrote. */
export function install(src, dest) {
  const next = fs.readFileSync(src);
  let prev;
  try {
    prev = fs.readFileSync(dest);
  } catch {
    prev = undefined;
  }
  if (prev?.equals(next)) return false;
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  const tmp = `${dest}.tmp-${process.pid}`;
  fs.writeFileSync(tmp, next, { mode: 0o755 });
  fs.chmodSync(tmp, 0o755);
  const now = new Date();
  fs.utimesSync(tmp, now, now);
  fs.renameSync(tmp, dest);
  return true;
}

/** The copy `tauri dev` runs: <target>/debug/<name>, synced from the externalBin if that debug dir exists. */
export function syncDevCopy(externalBin, name, targetDir) {
  const debug = path.join(targetDir, 'debug');
  if (!fs.existsSync(debug)) return false;
  return install(externalBin, path.join(debug, name));
}

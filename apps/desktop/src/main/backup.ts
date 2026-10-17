import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import AdmZip from 'adm-zip';
import { z } from 'zod';
import { AppError } from '@qrtz/core';

export const BACKUP_FORMAT = 'qrtz-backup';
export const DB_ENTRY = 'db/qrtz.db';

export const BackupManifest = z.object({
  format: z.literal(BACKUP_FORMAT),
  version: z.literal(1),
  createdAt: z.number(),
  appVersion: z.string(),
  schemaVersion: z.number().int(),
  meetings: z.number().int(),
  files: z
    .array(
      z.object({
        path: z.string(),
        sha256: z.string().regex(/^[0-9a-f]{64}$/),
        bytes: z.number().int().nonnegative(),
      }),
    )
    .max(100_000),
});
export type BackupManifest = z.infer<typeof BackupManifest>;

const sha256 = (b: Buffer) => createHash('sha256').update(b).digest('hex');
/** Only these entry names are ever written or read (no traversal, no absolute paths). */
const SAFE_AUDIO = /^audio\/[A-Za-z0-9._-]{1,128}\.wav$/;
const isSafeEntry = (p: string) => p === DB_ENTRY || SAFE_AUDIO.test(p);

/** Zip a DB snapshot and audio files with a sha256 manifest. */
export function writeBackup(o: {
  target: string;
  dbSnapshot: string;
  audioFiles: string[];
  appVersion: string;
  schemaVersion: number;
  meetings: number;
  now?: number;
}): { bytes: number; audioFiles: number } {
  const zip = new AdmZip();
  const files: BackupManifest['files'] = [];
  const add = (entry: string, data: Buffer) => {
    zip.addFile(entry, data);
    files.push({ path: entry, sha256: sha256(data), bytes: data.length });
  };
  add(DB_ENTRY, fs.readFileSync(o.dbSnapshot));
  let audio = 0;
  for (const f of o.audioFiles) {
    const entry = `audio/${path.basename(f)}`;
    if (!SAFE_AUDIO.test(entry) || !fs.existsSync(f)) continue;
    add(entry, fs.readFileSync(f));
    audio++;
  }
  const manifest: BackupManifest = {
    format: BACKUP_FORMAT,
    version: 1,
    createdAt: o.now ?? Date.now(),
    appVersion: o.appVersion,
    schemaVersion: o.schemaVersion,
    meetings: o.meetings,
    files,
  };
  zip.addFile('manifest.json', Buffer.from(JSON.stringify(manifest, null, 2)));
  fs.mkdirSync(path.dirname(o.target), { recursive: true });
  const tmp = `${o.target}.partial`;
  zip.writeZip(tmp);
  fs.renameSync(tmp, o.target);
  return { bytes: fs.statSync(o.target).size, audioFiles: audio };
}

/**
 * Verifies a backup and extracts it into `workDir` (DB) and `audioDir` (recordings). Throws E_VALIDATION on any
 * mismatch: missing/extra entries, bad hashes, unsafe names. Existing audio files with the same name are kept
 * when identical, otherwise the restored copy gets a suffix.
 */
export function readBackup(o: {
  source: string;
  workDir: string;
  audioDir: string;
}): {
  manifest: BackupManifest;
  dbPath: string;
  audioMap: Record<string, string>;
} {
  const bad = (msg: string) =>
    new AppError('E_VALIDATION', `Backup rejected: ${msg}`);
  let zip: AdmZip;
  try {
    zip = new AdmZip(o.source);
  } catch {
    throw bad('not a zip file');
  }
  const mEntry = zip.getEntry('manifest.json');
  if (!mEntry) throw bad('manifest.json missing');
  let manifest: BackupManifest;
  try {
    manifest = BackupManifest.parse(
      JSON.parse(mEntry.getData().toString('utf8')),
    );
  } catch {
    throw bad('manifest is invalid');
  }
  const listed = new Set(manifest.files.map((f) => f.path));
  if (!listed.has(DB_ENTRY)) throw bad('database missing from manifest');
  for (const e of zip.getEntries()) {
    if (e.isDirectory || e.entryName === 'manifest.json') continue;
    if (!listed.has(e.entryName)) throw bad(`unexpected file ${e.entryName}`);
  }
  const datas = new Map<string, Buffer>();
  for (const f of manifest.files) {
    if (!isSafeEntry(f.path)) throw bad(`unsafe path ${f.path}`);
    const e = zip.getEntry(f.path);
    if (!e) throw bad(`${f.path} missing`);
    let data: Buffer;
    try {
      data = e.getData();
    } catch {
      throw bad(`${f.path} is corrupt and failed its integrity check`);
    }
    if (data.length !== f.bytes || sha256(data) !== f.sha256)
      throw bad(`${f.path} failed its integrity check`);
    datas.set(f.path, data);
  }
  fs.mkdirSync(o.workDir, { recursive: true });
  fs.mkdirSync(o.audioDir, { recursive: true });
  const dbPath = path.join(o.workDir, 'import.db');
  fs.writeFileSync(dbPath, datas.get(DB_ENTRY)!);
  const audioMap: Record<string, string> = {};
  for (const [entry, data] of datas) {
    if (entry === DB_ENTRY) continue;
    const name = path.basename(entry);
    let target = path.join(o.audioDir, name);
    if (
      fs.existsSync(target) &&
      sha256(fs.readFileSync(target)) !== sha256(data)
    )
      target = path.join(
        o.audioDir,
        name.replace(/\.wav$/, `-restored-${Date.now()}.wav`),
      );
    if (!fs.existsSync(target)) fs.writeFileSync(target, data);
    audioMap[name] = target;
  }
  return { manifest, dbPath, audioMap };
}

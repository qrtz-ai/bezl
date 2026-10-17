import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import yazl from 'yazl';
import yauzl from 'yauzl';
import { z } from 'zod';
import { AppError } from '@qrtz/core';

/**
 * Streaming backup format (constant memory, any library size):
 *   db/qrtz.db            consistent VACUUM INTO snapshot
 *   audio/<name>.wav      recordings (stored, not deflated: WAV barely compresses)
 *   manifest.json         sha256 + size of every other entry
 */
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

/** Only these entry names are ever written or read (no traversal, no absolute paths). */
const SAFE_AUDIO = /^audio\/[A-Za-z0-9._-]{1,128}\.wav$/;
const isSafeEntry = (p: string) => p === DB_ENTRY || SAFE_AUDIO.test(p);
const MAX_MANIFEST = 16 * 1024 * 1024;

/** Streams a file through sha256 (no buffering). */
export async function hashFile(
  file: string,
): Promise<{ sha256: string; bytes: number }> {
  const h = createHash('sha256');
  let bytes = 0;
  for await (const chunk of fs.createReadStream(file)) {
    h.update(chunk as Buffer);
    bytes += (chunk as Buffer).length;
  }
  return { sha256: h.digest('hex'), bytes };
}

/** Zip a DB snapshot and audio files with a sha256 manifest, streaming both hashing and writing. */
export async function writeBackup(o: {
  target: string;
  dbSnapshot: string;
  audioFiles: string[];
  appVersion: string;
  schemaVersion: number;
  meetings: number;
  now?: number;
}): Promise<{ bytes: number; audioFiles: number }> {
  const entries: { entry: string; file: string }[] = [
    { entry: DB_ENTRY, file: o.dbSnapshot },
  ];
  for (const f of o.audioFiles) {
    const entry = `audio/${path.basename(f)}`;
    if (SAFE_AUDIO.test(entry) && fs.existsSync(f))
      entries.push({ entry, file: f });
  }
  const files: BackupManifest['files'] = [];
  for (const e of entries)
    files.push({ path: e.entry, ...(await hashFile(e.file)) });
  const manifest: BackupManifest = {
    format: BACKUP_FORMAT,
    version: 1,
    createdAt: o.now ?? Date.now(),
    appVersion: o.appVersion,
    schemaVersion: o.schemaVersion,
    meetings: o.meetings,
    files,
  };
  const zip = new yazl.ZipFile();
  for (const e of entries)
    zip.addReadStream(fs.createReadStream(e.file), e.entry, {
      compress: e.entry === DB_ENTRY,
    });
  zip.addBuffer(
    Buffer.from(JSON.stringify(manifest, null, 2)),
    'manifest.json',
  );
  zip.end();
  fs.mkdirSync(path.dirname(o.target), { recursive: true });
  const tmp = `${o.target}.partial`;
  try {
    await pipeline(zip.outputStream, fs.createWriteStream(tmp));
    fs.renameSync(tmp, o.target);
  } catch (e) {
    fs.rmSync(tmp, { force: true });
    throw e;
  }
  return { bytes: fs.statSync(o.target).size, audioFiles: entries.length - 1 };
}

type Zip = yauzl.ZipFile;
const openZip = (file: string) =>
  new Promise<Zip>((res, rej) =>
    yauzl.open(
      file,
      { lazyEntries: true, autoClose: false, strictFileNames: true },
      (e, z) => (e || !z ? rej(e ?? new Error('zip')) : res(z)),
    ),
  );
const listEntries = (zip: Zip) =>
  new Promise<yauzl.Entry[]>((res, rej) => {
    const out: yauzl.Entry[] = [];
    zip.on('entry', (e: yauzl.Entry) => {
      out.push(e);
      zip.readEntry();
    });
    zip.once('end', () => res(out));
    zip.once('error', rej);
    zip.readEntry();
  });
const entryStream = (zip: Zip, e: yauzl.Entry) =>
  new Promise<NodeJS.ReadableStream>((res, rej) =>
    zip.openReadStream(e, (err, s) =>
      err || !s ? rej(err ?? new Error('stream')) : res(s),
    ),
  );

/**
 * Verifies a backup while streaming it out: `workDir` gets the DB, `audioDir` the recordings. Throws
 * E_VALIDATION on any mismatch (missing/extra entries, bad hashes or sizes, unsafe names, corrupt data) and
 * removes everything it wrote. Existing identical audio is kept; a different file with the same name is
 * restored under a suffixed name.
 */
export async function readBackup(o: {
  source: string;
  workDir: string;
  audioDir: string;
}): Promise<{
  manifest: BackupManifest;
  dbPath: string;
  audioMap: Record<string, string>;
}> {
  const bad = (msg: string) =>
    new AppError('E_VALIDATION', `Backup rejected: ${msg}`);
  let zip: Zip;
  try {
    zip = await openZip(o.source);
  } catch {
    throw bad('not a zip file');
  }
  const written: string[] = [];
  try {
    let entries: yauzl.Entry[];
    try {
      entries = await listEntries(zip);
    } catch (e) {
      throw bad(`unreadable zip (${(e as Error).message})`);
    }
    const byName = new Map(entries.map((e) => [e.fileName, e]));
    const mEntry = byName.get('manifest.json');
    if (!mEntry || mEntry.uncompressedSize > MAX_MANIFEST)
      throw bad('manifest.json missing');
    let manifest: BackupManifest;
    try {
      const chunks: Buffer[] = [];
      for await (const c of (await entryStream(
        zip,
        mEntry,
      )) as AsyncIterable<Buffer>)
        chunks.push(c);
      manifest = BackupManifest.parse(
        JSON.parse(Buffer.concat(chunks).toString('utf8')),
      );
    } catch {
      throw bad('manifest is invalid');
    }
    const listed = new Map(manifest.files.map((f) => [f.path, f]));
    if (!listed.has(DB_ENTRY)) throw bad('database missing from manifest');
    for (const e of entries) {
      if (e.fileName.endsWith('/') || e.fileName === 'manifest.json') continue;
      if (!listed.has(e.fileName)) throw bad(`unexpected file ${e.fileName}`);
    }
    fs.mkdirSync(o.workDir, { recursive: true });
    fs.mkdirSync(o.audioDir, { recursive: true });
    const dbPath = path.join(o.workDir, 'import.db');
    const staged: { entry: string; tmp: string; sha256: string }[] = [];
    for (const f of manifest.files) {
      if (!isSafeEntry(f.path)) throw bad(`unsafe path ${f.path}`);
      const e = byName.get(f.path);
      if (!e) throw bad(`${f.path} missing`);
      if (e.uncompressedSize !== f.bytes)
        throw bad(`${f.path} failed its integrity check`);
      const tmp =
        f.path === DB_ENTRY
          ? dbPath
          : path.join(
              o.audioDir,
              `.${path.basename(f.path)}.${process.pid}.partial`,
            );
      written.push(tmp);
      const h = createHash('sha256');
      let bytes = 0;
      const tap = new Transform({
        transform(chunk: Buffer, _enc, cb) {
          h.update(chunk);
          bytes += chunk.length;
          if (bytes > f.bytes)
            return cb(new Error('entry larger than its manifest size'));
          cb(null, chunk);
        },
      });
      try {
        await pipeline(
          await entryStream(zip, e),
          tap,
          fs.createWriteStream(tmp),
        );
      } catch {
        throw bad(`${f.path} is corrupt and failed its integrity check`);
      }
      const sha256 = h.digest('hex');
      if (bytes !== f.bytes || sha256 !== f.sha256)
        throw bad(`${f.path} failed its integrity check`);
      if (f.path !== DB_ENTRY) staged.push({ entry: f.path, tmp, sha256 });
    }
    // Everything verified: move audio into place.
    const audioMap: Record<string, string> = {};
    for (const s of staged) {
      const name = path.basename(s.entry);
      let target = path.join(o.audioDir, name);
      if (fs.existsSync(target) && (await hashFile(target)).sha256 !== s.sha256)
        target = path.join(
          o.audioDir,
          name.replace(/\.wav$/, `-restored-${Date.now()}.wav`),
        );
      if (fs.existsSync(target)) fs.rmSync(s.tmp, { force: true });
      else fs.renameSync(s.tmp, target);
      audioMap[name] = target;
    }
    return { manifest, dbPath, audioMap };
  } catch (e) {
    for (const f of written) fs.rmSync(f, { force: true });
    throw e;
  } finally {
    zip.close();
  }
}

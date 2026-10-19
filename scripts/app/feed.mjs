#!/usr/bin/env node
// ADR 0181: qrtz.app's update feed, v1/app/latest.json on modules.qrtz.ai. Tauri's static-feed shape plus the
// archive's sha256 and size (the app checks both before the signature; crates/qrtz-app/src/self_update.rs parses it
// strictly: the url must be exactly the immutable versioned archive on the same host).
//
//   node scripts/app/feed.mjs --version 0.2.1 --archive <tar.gz> --sig <tar.gz.sig> [--notes-file f] [--host url]
//     prints latest.json on stdout
//   node scripts/app/feed.mjs --check-key <app_update_key.pub>
//     exit 0 if the committed public key file holds one key, 1 if it is still the TODO
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

export const HOST = 'https://modules.qrtz.ai';
export const TARGET = 'darwin-aarch64';
export const archivePath = (v) => `v1/app/${v}/qrtz_${v}_aarch64.app.tar.gz`;

/** The public key in the committed file (lines starting `#` are comments), or null. */
export function keyFrom(text) {
  const lines = text
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith('#'));
  if (lines.length !== 1) return null;
  const k = lines[0];
  if (!/^[A-Za-z0-9+/]+=*$/.test(k)) return null;
  const decoded = Buffer.from(k, 'base64').toString('utf8');
  return decoded.includes('minisign public key') ? k : null;
}

export function feed({
  version,
  archive,
  sig,
  notes,
  host = HOST,
  date = new Date(),
}) {
  if (!/^\d+\.\d+\.\d+$/.test(version))
    throw new Error(`not a plain x.y.z version: ${version}`);
  const bytes = fs.readFileSync(archive);
  const signature = fs.readFileSync(sig, 'utf8').trim();
  if (!signature) throw new Error(`${sig}: empty`);
  return {
    version,
    notes: (notes ?? '').slice(0, 4000),
    pub_date: date.toISOString().replace(/\.\d{3}Z$/, 'Z'),
    platforms: {
      [TARGET]: {
        url: `${host}/${archivePath(version)}`,
        signature,
        sha256: createHash('sha256').update(bytes).digest('hex'),
        size: bytes.length,
      },
    },
  };
}

function main(argv) {
  const a = {};
  for (let i = 0; i < argv.length; i += 2)
    a[argv[i].replace(/^--/, '')] = argv[i + 1];
  if (a['check-key']) {
    const k = keyFrom(fs.readFileSync(a['check-key'], 'utf8'));
    console.error(
      k
        ? 'feed: the app-update public key is set'
        : 'feed: no app-update public key yet (TODO)',
    );
    process.exit(k ? 0 : 1);
  }
  const notes = a['notes-file']
    ? fs.readFileSync(a['notes-file'], 'utf8').trim()
    : '';
  const f = feed({
    version: a.version,
    archive: a.archive,
    sig: a.sig,
    notes,
    host: a.host ?? HOST,
  });
  process.stdout.write(JSON.stringify(f, null, 2) + '\n');
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(import.meta.filename)
) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`feed: ${e.message}`);
    process.exit(1);
  }
}

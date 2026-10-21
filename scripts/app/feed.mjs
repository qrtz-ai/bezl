#!/usr/bin/env node
// ADR 0181: qrtz.app's update feed, latest.json, a GitHub release asset of qrtz-ai/qrtz (QRT-313; the app reads
// https://github.com/qrtz-ai/qrtz/releases/latest/download/latest.json). The feed is Tauri's static-feed shape
// plus the archive's sha256 and size and an `expires` time (the app refuses an expired feed: a replayed old one can't
// hide updates for long). latest.json is a signed envelope around the feed's exact bytes:
// {"feed": <base64>, "signature": <base64 minisign, the app-update key>}; crates/qrtz-app/src/self_update.rs opens it
// and parses the feed strictly (the url must be exactly the versioned archive asset of release app-v<version>).
//
//   node scripts/app/feed.mjs --version 0.2.1 --archive <tar.gz> --sig <tar.gz.sig> [--notes-file f] [--host url]
//                             [--expires YYYY-MM-DDTHH:MM:SSZ | --expires-days 30]
//     prints the feed on stdout (sign that file, then --wrap it)
//   node scripts/app/feed.mjs --wrap <feed.json> --wrap-sig <feed.json.sig>
//     prints latest.json (the envelope) on stdout
//   node scripts/app/feed.mjs --unwrap <latest.json> [--expires-days 30]
//     prints the feed inside an envelope, with a new expires if asked (app-feed-refresh.yml re-signs it)
//   node scripts/app/feed.mjs --check-key <app_update_key.pub>
//     exit 0 if the committed public key file holds one key, 1 if it is still the TODO
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

/** QRT-313: archives are assets of release `app-v<version>` (self_update.rs ARCHIVE_BASE). */
export const HOST = 'https://github.com/qrtz-ai/qrtz/releases/download';
export const TARGET = 'darwin-aarch64';
export const archivePath = (v) => `app-v${v}/qrtz_${v}_aarch64.app.tar.gz`;
/** The app refuses a feed whose expires is further ahead than this (self_update.rs MAX_FEED_LIFETIME). */
export const MAX_LIFETIME_DAYS = 60;
export const DEFAULT_LIFETIME_DAYS = 30;

const utc = (d) => d.toISOString().replace(/\.\d{3}Z$/, 'Z');

/** `days` after `now`, as the app parses it (YYYY-MM-DDTHH:MM:SSZ). */
export function expiresIn(days, now = new Date()) {
  const n = Number(days);
  if (!(n > 0 && n <= MAX_LIFETIME_DAYS))
    throw new Error(`expires must be 1..${MAX_LIFETIME_DAYS} days ahead`);
  return utc(new Date(now.getTime() + n * 86400 * 1000));
}

/** latest.json: the feed's exact bytes and their signature (the .sig `tauri signer sign` wrote, base64). */
export function wrap(feedBytes, sigText) {
  const signature = sigText.trim();
  if (!signature) throw new Error('empty feed signature');
  JSON.parse(feedBytes.toString('utf8'));
  return { feed: Buffer.from(feedBytes).toString('base64'), signature };
}

/** The feed inside latest.json (not verified here; the app verifies). */
export function unwrap(envelope) {
  const e = JSON.parse(envelope);
  if (typeof e.feed !== 'string') throw new Error('latest.json has no feed');
  return JSON.parse(Buffer.from(e.feed, 'base64').toString('utf8'));
}

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
  expires = expiresIn(DEFAULT_LIFETIME_DAYS, date),
}) {
  if (!/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/.test(expires))
    throw new Error(`expires isn't YYYY-MM-DDTHH:MM:SSZ: ${expires}`);
  if (!/^\d+\.\d+\.\d+$/.test(version))
    throw new Error(`not a plain x.y.z version: ${version}`);
  const bytes = fs.readFileSync(archive);
  const signature = fs.readFileSync(sig, 'utf8').trim();
  if (!signature) throw new Error(`${sig}: empty`);
  return {
    version,
    notes: (notes ?? '').slice(0, 4000),
    pub_date: utc(date),
    expires,
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
  if (a.wrap) {
    const env = wrap(
      fs.readFileSync(a.wrap),
      fs.readFileSync(a['wrap-sig'], 'utf8'),
    );
    process.stdout.write(JSON.stringify(env, null, 2) + '\n');
    return;
  }
  if (a.unwrap) {
    const f = unwrap(fs.readFileSync(a.unwrap, 'utf8'));
    if (a['expires-days']) f.expires = expiresIn(a['expires-days']);
    process.stdout.write(JSON.stringify(f, null, 2) + '\n');
    return;
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
    ...(a.expires
      ? { expires: a.expires }
      : a['expires-days']
        ? { expires: expiresIn(a['expires-days']) }
        : {}),
  });
  process.stdout.write(JSON.stringify(f, null, 2) + '\n');
}

if (
  process.argv[1] &&
  // realpath: on macOS a script under /var (a symlink to /private/var) has argv[1] and import.meta.filename apart.
  fs.realpathSync(path.resolve(process.argv[1])) ===
    fs.realpathSync(import.meta.filename)
) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`feed: ${e.message}`);
    process.exit(1);
  }
}

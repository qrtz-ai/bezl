#!/usr/bin/env node
// ADR 0218: the module store as assets of the one GitHub release `modules-index` of qrtz-ai/qrtz. A release has no
// directories, so each layout path is one flat asset name: every `/` becomes `.` (the same map as
// crates/qrtz-app/src/module_hosting.rs `flat_asset_name`):
//
//   v1/root/<n>.root.json[.sig]     v1.root.<n>.root.json[.sig]
//   v1/timestamp.json[.sig]         v1.timestamp.json[.sig]
//   v1/index/<sha256>.json[.sig]    v1.index.<sha256>.json[.sig]
//   v1/blobs/sha256/<sha256>        v1.blobs.sha256.<sha256>
//
//   node scripts/modules/gh-assets.mjs name <layout path>             print the asset name
//   node scripts/modules/gh-assets.mjs path <asset name>              print the layout path (refuses anything else)
//   node scripts/modules/gh-assets.mjs hydrate <out dir>              fill <out>/v1 from the release; when the release
//        doesn't exist yet (the first run), seed it from $QRTZ_SEED_URL (https://modules.qrtz.ai): the root(s), the
//        timestamp pair, the index pair and every blob the index names (model hashes 404 there and are skipped:
//        model bytes come from Hugging Face, ADR 0171). The output is then checked by `qrtz-publish verify`.
//   node scripts/modules/gh-assets.mjs upload <out dir> [--timestamp-only]   upload what the release lacks, in
//        `qrtz-publish upload-list` order; immutable objects are never replaced (an existing asset with other bytes
//        fails the run), the timestamp pair goes last with --clobber.
//        With no release yet, either mode creates it (--latest=false) and uploads the whole seeded out dir (QRT-343).
//
// Env: GH_TOKEN (contents: write for upload/create), GITHUB_REPOSITORY (qrtz-ai/qrtz), QRTZ_PUBLISH_BIN,
// QRTZ_MODULES_RELEASE (modules-index), QRTZ_SEED_URL. Uses the `gh` CLI. No secret is ever printed.
import { execFileSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export const RELEASE = process.env.QRTZ_MODULES_RELEASE || 'modules-index';

export function assetName(p) {
  if (!layoutPath(p.replaceAll('/', '.')))
    throw new Error(`${p}: not a layout path`);
  return p.replaceAll('/', '.');
}

/** The layout path of a flat asset name, or null for anything that isn't one. */
export function layoutPath(name) {
  const m =
    /^v1\.(timestamp\.json(?:\.sig)?)$/.exec(name) ||
    /^v1\.root\.([1-9][0-9]*\.root\.json(?:\.sig)?)$/.exec(name) ||
    /^v1\.index\.([0-9a-f]{64}\.json(?:\.sig)?)$/.exec(name) ||
    /^v1\.blobs\.sha256\.([0-9a-f]{64})$/.exec(name);
  if (!m) return null;
  if (name.startsWith('v1.timestamp')) return `v1/${m[1]}`;
  if (name.startsWith('v1.root.')) return `v1/root/${m[1]}`;
  if (name.startsWith('v1.index.')) return `v1/index/${m[1]}`;
  return `v1/blobs/sha256/${m[1]}`;
}

const repo = () => process.env.GITHUB_REPOSITORY || 'qrtz-ai/qrtz';
const gh = (args, opts = {}) =>
  execFileSync('gh', args, {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'inherit'],
    ...opts,
  });

function releaseAssets() {
  try {
    const j = JSON.parse(
      gh(['release', 'view', RELEASE, '-R', repo(), '--json', 'assets'], {
        stdio: ['ignore', 'pipe', 'ignore'],
      }),
    );
    return new Map(j.assets.map((a) => [a.name, a]));
  } catch {
    return null; // no such release
  }
}

async function get(url) {
  const r = await fetch(url, { headers: { 'cache-control': 'no-cache' } });
  if (r.status === 404) return null;
  if (!r.ok) throw new Error(`${url}: http ${r.status}`);
  return Buffer.from(await r.arrayBuffer());
}

async function seed(out, base) {
  const put = (p, b) => {
    fs.mkdirSync(path.dirname(path.join(out, p)), { recursive: true });
    fs.writeFileSync(path.join(out, p), b);
  };
  const need = async (p) => {
    const b = await get(`${base}/${p}`);
    if (!b) throw new Error(`${base}/${p}: missing`);
    put(p, b);
    return b;
  };
  for (let n = 1; ; n++) {
    const b = await get(`${base}/v1/root/${n}.root.json`);
    if (!b) break;
    put(`v1/root/${n}.root.json`, b);
    await need(`v1/root/${n}.root.json.sig`);
  }
  const ts = JSON.parse(await need('v1/timestamp.json'));
  await need('v1/timestamp.json.sig');
  const ix = ts.index.sha256;
  const index = await need(`v1/index/${ix}.json`);
  await need(`v1/index/${ix}.json.sig`);
  let n = 0;
  for (const h of new Set(
    index.toString('utf8').match(/[0-9a-f]{64}/g) || [],
  )) {
    const b = await get(`${base}/v1/blobs/sha256/${h}`);
    if (b) {
      if (crypto.createHash('sha256').update(b).digest('hex') !== h)
        throw new Error(`blob ${h}: wrong bytes`);
      put(`v1/blobs/sha256/${h}`, b);
      n++;
    }
  }
  console.log(
    `gh-assets: seeded ${out} from ${base} (index ${ix}, ${n} blobs)`,
  );
}

async function hydrate(out) {
  fs.mkdirSync(path.join(out, 'v1'), { recursive: true });
  const assets = releaseAssets();
  if (!assets) {
    await seed(out, process.env.QRTZ_SEED_URL || 'https://modules.qrtz.ai');
    return;
  }
  const dl = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-gh-assets-'));
  try {
    gh([
      'release',
      'download',
      RELEASE,
      '-R',
      repo(),
      '-D',
      dl,
      '--pattern',
      'v1.*',
    ]);
    let n = 0;
    for (const name of fs.readdirSync(dl)) {
      const p = layoutPath(name);
      if (!p) continue;
      fs.mkdirSync(path.dirname(path.join(out, p)), { recursive: true });
      fs.copyFileSync(path.join(dl, name), path.join(out, p));
      n++;
    }
    console.log(`gh-assets: ${out} mirrors release ${RELEASE} (${n} objects)`);
  } finally {
    fs.rmSync(dl, { recursive: true, force: true });
  }
}

/**
 * The `qrtz-publish upload-list` arguments. --timestamp-only narrows the list to the timestamp pair only while the
 * release exists; the first upload (no release yet, QRT-343) lists the whole out dir, which hydrate seeded from
 * modules.qrtz.ai and `qrtz-publish timestamp` verified against root 1, so the release starts as a full mirror.
 */
export function uploadListArgs(out, timestampOnly, releaseExists) {
  return [
    'upload-list',
    '--out',
    out,
    ...(timestampOnly && releaseExists ? ['--timestamp-only'] : []),
  ];
}

function upload(out, timestampOnly) {
  const bin = process.env.QRTZ_PUBLISH_BIN || 'qrtz-publish';
  let assets = releaseAssets();
  const list = execFileSync(
    bin,
    uploadListArgs(out, timestampOnly, assets !== null),
    {
      encoding: 'utf8',
    },
  )
    .trim()
    .split('\n')
    .map((l) => l.split('\t'));
  if (!assets) {
    gh([
      'release',
      'create',
      RELEASE,
      '-R',
      repo(),
      '--latest=false',
      '--title',
      'qrtz module index',
      '--notes',
      'The signed qrtz module index (ADR 0218). Assets are content-addressed; do not edit by hand.',
    ]);
    assets = new Map();
  }
  const stage = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-gh-up-'));
  try {
    const pair = [];
    let n = 0;
    for (const [key, , cc] of list) {
      const name = assetName(key);
      const file = path.join(stage, name);
      fs.copyFileSync(path.join(out, key), file);
      if (!cc.includes('immutable')) {
        pair.push(file);
        continue;
      }
      const have = assets.get(name);
      if (have) {
        if (have.size !== fs.statSync(file).size)
          throw new Error(
            `${name}: already in ${RELEASE} with other bytes (immutable)`,
          );
        continue;
      }
      gh(['release', 'upload', RELEASE, '-R', repo(), file]);
      n++;
    }
    // The pair last, together (.sig first in the list), replacing the old pair.
    if (pair.length)
      gh(['release', 'upload', RELEASE, '-R', repo(), '--clobber', ...pair]);
    console.log(
      `gh-assets: uploaded ${n} new immutable object(s) and the timestamp pair to ${RELEASE}`,
    );
  } finally {
    fs.rmSync(stage, { recursive: true, force: true });
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [cmd, a, b] = process.argv.slice(2);
  try {
    if (cmd === 'name') console.log(assetName(a));
    else if (cmd === 'path') {
      const p = layoutPath(a);
      if (!p) throw new Error(`${a}: not a module asset name`);
      console.log(p);
    } else if (cmd === 'hydrate' && a) await hydrate(a);
    else if (cmd === 'upload' && a) upload(a, b === '--timestamp-only');
    else {
      console.error(
        'usage: gh-assets.mjs name <path> | path <name> | hydrate <out> | upload <out> [--timestamp-only]',
      );
      process.exit(2);
    }
  } catch (e) {
    console.error(`gh-assets: ${e.message}`);
    process.exit(1);
  }
}

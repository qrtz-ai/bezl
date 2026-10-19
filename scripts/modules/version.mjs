#!/usr/bin/env node
// Module versions (nano, ctrl, snap; echo ships with the app, ADR 0174). Each module's release is the git tag
// `<module>-v<version>`. Before a publish, a module whose code or manifest changed since its last tag gets its patch
// number bumped in modules/<m>/qrtz.json and module.json, its modules.lock.json entry (version + manifestSha256 =
// sha256 of module.json), and a CHANGELOG entry from the commit subjects since the tag. After the upload, `tag`
// tags HEAD with each module's version and pushes the tags.
//
//   node scripts/modules/version.mjs plan  [--force] [m...]   print `<m>\t<old>\t<new|->` (no writes)
//   node scripts/modules/version.mjs bump  [--force] [m...]   write bumps + CHANGELOGs, print the files changed
//   node scripts/modules/version.mjs tag   [--push] [m...]    tag HEAD `<m>-v<version>` (refuses a tag at another commit)
//
// --force bumps a module even with no changes. A module whose qrtz.json version is already ahead of its last tag
// (bumped by an earlier run whose publish didn't finish) is not bumped again.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

export const MODULES = ['nano', 'ctrl', 'snap'];

const SHARED = [
  'native/qrtz-helper/Package.swift',
  'native/qrtz-helper/Package.resolved',
  'native/qrtz-helper/Sources/QrtzShared',
  'native/qrtz-helper/Sources/QrtzMacShared',
  'native/qrtz-helper/Sources/QrtzHelperCore',
];
const OWN = {
  nano: ['QrtzNano', 'QrtzNanoMac', 'qrtz-nano'],
  ctrl: ['QrtzCtrl', 'qrtz-ctrl'],
  snap: ['QrtzSnap', 'qrtz-snap'],
};

/** The repo paths whose change means a new release of module `m`. */
export function paths(m) {
  return [
    `modules/${m}/qrtz.json`,
    `modules/${m}/module.json`,
    `native/qrtz-helper/entitlements/ai.qrtz.${m}.entitlements`,
    ...OWN[m].map((d) => `native/qrtz-helper/Sources/${d}`),
    ...SHARED,
  ];
}

const git = (repo, args) =>
  execFileSync('git', args, {
    cwd: repo,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();

export function parseVersion(v) {
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(v);
  if (!m) throw new Error(`not a plain x.y.z version: ${v}`);
  return m.slice(1).map(Number);
}
export const cmpVersion = (a, b) => {
  const x = parseVersion(a),
    y = parseVersion(b);
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] - y[i];
  return 0;
};
export const bumpPatch = (v) => {
  const [a, b, c] = parseVersion(v);
  return `${a}.${b}.${c + 1}`;
};

/** The highest `<m>-v<x.y.z>` tag, as { tag, version }, or null. */
export function lastTag(repo, m) {
  const tags = git(repo, ['tag', '--list', `${m}-v*`])
    .split('\n')
    .filter(Boolean);
  const vs = tags
    .map((t) => t.slice(m.length + 2))
    .filter((v) => /^\d+\.\d+\.\d+$/.test(v))
    .sort(cmpVersion);
  if (!vs.length) return null;
  const version = vs[vs.length - 1];
  return { tag: `${m}-v${version}`, version };
}

const readJson = (f) => JSON.parse(fs.readFileSync(f, 'utf8'));
export const currentVersion = (repo, m) =>
  readJson(path.join(repo, 'modules', m, 'qrtz.json')).version;

/** What `bump` would do for module m: { module, from, to (null = no bump), reason, since }. */
export function planOne(repo, m, force = false) {
  const from = currentVersion(repo, m);
  const last = lastTag(repo, m);
  if (!last)
    return {
      module: m,
      from,
      to: null,
      reason: 'no release tag yet (tag the published version first)',
    };
  const c = cmpVersion(from, last.version);
  if (c > 0)
    return {
      module: m,
      from,
      to: null,
      reason: `already ahead of ${last.tag}`,
      since: last.tag,
    };
  if (c < 0)
    throw new Error(`${m}: qrtz.json ${from} is behind its tag ${last.tag}`);
  const changed =
    git(repo, ['diff', '--name-only', last.tag, 'HEAD', '--', ...paths(m)]) !==
    '';
  if (!changed && !force)
    return {
      module: m,
      from,
      to: null,
      reason: `unchanged since ${last.tag}`,
      since: last.tag,
    };
  return {
    module: m,
    from,
    to: bumpPatch(from),
    reason: changed ? 'changed' : 'forced',
    since: last.tag,
  };
}

/** Commit subjects touching m since `since`, newest first. */
export function subjects(repo, m, since) {
  const out = git(repo, [
    'log',
    '--format=%h %s',
    `${since}..HEAD`,
    '--',
    ...paths(m),
  ]);
  return out ? out.split('\n') : [];
}

function setVersion(file, to) {
  const s = fs.readFileSync(file, 'utf8');
  const re = /("version":\s*")(\d+\.\d+\.\d+)(")/;
  if (!re.test(s)) throw new Error(`${file}: no "version"`);
  fs.writeFileSync(file, s.replace(re, `$1${to}$3`));
}

export function changelogEntry(m, to, subs, date) {
  const lines = subs.length
    ? subs.map((l) => {
        const [h, ...rest] = l.split(' ');
        let s = rest.join(' ');
        if (s.length > 160) s = s.slice(0, 157).trimEnd() + '…';
        return `- ${s} (${h})`;
      })
    : ['- no code changes; republished'];
  return `## ${to} (${date})\n\n${lines.join('\n')}\n`;
}

/** Apply one plan entry; returns the files written. */
export function applyBump(
  repo,
  p,
  date = new Date().toISOString().slice(0, 10),
) {
  const dir = path.join(repo, 'modules', p.module);
  setVersion(path.join(dir, 'qrtz.json'), p.to);
  setVersion(path.join(dir, 'module.json'), p.to);
  const lockFile = path.join(repo, 'modules', 'modules.lock.json');
  const lock = readJson(lockFile);
  const e = lock.modules?.[p.module];
  if (!e) throw new Error(`modules.lock.json: no ${p.module}`);
  e.version = p.to;
  e.manifestSha256 = createHash('sha256')
    .update(fs.readFileSync(path.join(dir, 'module.json')))
    .digest('hex');
  fs.writeFileSync(lockFile, JSON.stringify(lock, null, 2) + '\n');
  const cl = path.join(dir, 'CHANGELOG.md');
  const head = `# ${p.module} changelog\n\n`;
  const old = fs.existsSync(cl)
    ? fs.readFileSync(cl, 'utf8').replace(head, '')
    : '';
  fs.writeFileSync(
    cl,
    head +
      changelogEntry(p.module, p.to, subjects(repo, p.module, p.since), date) +
      (old ? '\n' + old : ''),
  );
  return [
    `modules/${p.module}/qrtz.json`,
    `modules/${p.module}/module.json`,
    `modules/${p.module}/CHANGELOG.md`,
    'modules/modules.lock.json',
  ];
}

/** Tag HEAD `<m>-v<version>` for each module; an existing tag must already be at HEAD. Returns the tags. */
export function tagAll(repo, mods, push = false) {
  const headSha = git(repo, ['rev-parse', 'HEAD']);
  const made = [];
  for (const m of mods) {
    const t = `${m}-v${currentVersion(repo, m)}`;
    let at = '';
    try {
      at = git(repo, ['rev-parse', `${t}^{commit}`]);
    } catch {
      /* new */
    }
    if (at && at !== headSha) {
      if (
        git(repo, ['diff', '--name-only', at, 'HEAD', '--', ...paths(m)]) !== ''
      )
        throw new Error(
          `${t} exists at ${at.slice(0, 7)} with different code; bump ${m}`,
        );
      continue;
    }
    if (!at)
      git(repo, ['tag', '-a', t, '-m', `${m} ${currentVersion(repo, m)}`]);
    made.push(t);
  }
  if (push && made.length)
    git(repo, ['push', 'origin', ...made.map((t) => `refs/tags/${t}`)]);
  return made;
}

function main(argv) {
  const repo = path.resolve(import.meta.dirname, '../..');
  const [cmd, ...rest] = argv;
  const force = rest.includes('--force');
  const push = rest.includes('--push');
  const mods = rest.filter((a) => !a.startsWith('--'));
  const list = mods.length ? mods : MODULES;
  for (const m of list)
    if (!MODULES.includes(m))
      throw new Error(`${m}: not a versioned module (${MODULES.join(', ')})`);
  if (cmd === 'plan' || cmd === 'bump') {
    for (const m of list) {
      const p = planOne(repo, m, force);
      if (cmd === 'bump' && p.to)
        for (const f of applyBump(repo, p)) console.log(f);
      console.error(`version: ${m} ${p.from} -> ${p.to ?? '-'} (${p.reason})`);
      if (cmd === 'plan') console.log(`${m}\t${p.from}\t${p.to ?? '-'}`);
    }
  } else if (cmd === 'tag') {
    for (const t of tagAll(repo, list, push)) console.log(t);
  } else {
    throw new Error(
      'usage: version.mjs plan|bump [--force] [m...] | tag [--push] [m...]',
    );
  }
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(import.meta.filename)
) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`version: ${e.message}`);
    process.exit(1);
  }
}

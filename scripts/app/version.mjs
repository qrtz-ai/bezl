#!/usr/bin/env node
// qrtz.app's own version (ADR 0181). The shell's release is the git tag `app-v<version>` (the old `v<version>` tags of
// release.yml count as earlier releases). A change to the shell since its last tag gets a patch bump in every file
// that carries the app's version (tauri.conf.json, the root and desktop package.json, the workspace Cargo.toml and
// its Cargo.lock entries) and a CHANGELOG.md section: the "## Unreleased" notes become "## <version> (<date>)", or,
// when there are none, the commit subjects since the tag. app-release.yml releases a commit whose tauri.conf.json
// version went up, and tags it (`tag`).
//
//   node scripts/app/version.mjs plan  [--force]   print `app\t<old>\t<new|->` (no writes)
//   node scripts/app/version.mjs bump  [--force]   write the bump + CHANGELOG, print the files changed
//   node scripts/app/version.mjs tag   [--push]    tag HEAD `app-v<version>` (refuses a tag at another commit)
//   node scripts/app/version.mjs current           print the version in tauri.conf.json
//
// echo is part of the shell: qrtz.app carries qrtz-echo and it changes only with the whole signed bundle (ADR 0174;
// Bozniak's rule, ADR 0181), so a change to echo's code bumps qrtz. nano, ctrl and snap are their own releases
// (scripts/modules/version.mjs, ADR 0179) and never bump the shell.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { bumpPatch, cmpVersion, parseVersion } from '../modules/version.mjs';

export const TAG_PREFIX = 'app-v';
const CONF = 'apps/desktop/src-tauri/tauri.conf.json';
const PACKAGE_JSONS = ['package.json', 'apps/desktop/package.json'];

/** The paths whose change is a change to qrtz.app (git pathspecs, with the module-only code excluded). */
export const SHELL_PATHS = [
  'apps/desktop',
  'crates',
  'packages',
  'contracts',
  'native/qrtz-helper',
  'modules/echo',
  'modules/modules.lock.json',
  'Cargo.toml',
  'Cargo.lock',
  'package.json',
  'bun.lock',
  'rust-toolchain.toml',
  // Released on their own (scripts/modules/version.mjs): never the shell.
  ':(exclude)native/qrtz-helper/Sources/QrtzNano',
  ':(exclude)native/qrtz-helper/Sources/QrtzNanoMac',
  ':(exclude)native/qrtz-helper/Sources/qrtz-nano',
  ':(exclude)native/qrtz-helper/Sources/QrtzCtrl',
  ':(exclude)native/qrtz-helper/Sources/qrtz-ctrl',
  ':(exclude)native/qrtz-helper/Sources/QrtzSnap',
  ':(exclude)native/qrtz-helper/Sources/qrtz-snap',
  ':(exclude)native/qrtz-helper/entitlements/ai.qrtz.nano.entitlements',
  ':(exclude)native/qrtz-helper/entitlements/ai.qrtz.ctrl.entitlements',
  ':(exclude)native/qrtz-helper/entitlements/ai.qrtz.snap.entitlements',
  // Docs and tests-only notes don't make a release.
  ':(exclude)**/*.md',
];

const git = (repo, args) =>
  execFileSync('git', args, {
    cwd: repo,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();

const readJson = (f) => JSON.parse(fs.readFileSync(f, 'utf8'));
export const currentVersion = (repo) => readJson(path.join(repo, CONF)).version;

/** The newest shell release tag (`app-v*`, or an old `v*`), as { tag, version }, or null. */
export function lastTag(repo) {
  const tags = git(repo, ['tag', '--list', `${TAG_PREFIX}*`, 'v*'])
    .split('\n')
    .filter(Boolean)
    .map((t) => ({
      tag: t,
      version: t.startsWith(TAG_PREFIX)
        ? t.slice(TAG_PREFIX.length)
        : t.slice(1),
    }))
    .filter((t) => /^\d+\.\d+\.\d+$/.test(t.version))
    .sort(
      (a, b) =>
        cmpVersion(a.version, b.version) ||
        (a.tag.startsWith(TAG_PREFIX) ? 1 : -1),
    );
  return tags.length ? tags[tags.length - 1] : null;
}

/** The files under SHELL_PATHS that changed between `from` and `to`. */
export function shellChanges(repo, from, to = 'HEAD') {
  const out = git(repo, [
    'diff',
    '--name-only',
    from,
    to,
    '--',
    ...SHELL_PATHS,
  ]);
  return out ? out.split('\n') : [];
}

/** What `bump` would do: { from, to (null = no bump), reason, since }. */
export function plan(repo, force = false) {
  const from = currentVersion(repo);
  parseVersion(from);
  const last = lastTag(repo);
  if (!last)
    return {
      from,
      to: null,
      reason: 'no release tag yet (tag the released version first)',
    };
  const c = cmpVersion(from, last.version);
  if (c > 0)
    return {
      from,
      to: null,
      reason: `already ahead of ${last.tag}`,
      since: last.tag,
    };
  if (c < 0) throw new Error(`${CONF} ${from} is behind its tag ${last.tag}`);
  const changed = shellChanges(repo, last.tag).length > 0;
  if (!changed && !force)
    return {
      from,
      to: null,
      reason: `unchanged since ${last.tag}`,
      since: last.tag,
    };
  return {
    from,
    to: bumpPatch(from),
    reason: changed ? 'changed' : 'forced',
    since: last.tag,
  };
}

function replaceOnce(file, re, to) {
  const s = fs.readFileSync(file, 'utf8');
  if (!re.test(s)) throw new Error(`${file}: no version to bump`);
  fs.writeFileSync(file, s.replace(re, `$1${to}$3`));
}

/** The workspace members whose version is the workspace's (their Cargo.lock entries move with it). */
export function workspaceMembers(repo) {
  const files = git(repo, ['ls-files', '*Cargo.toml', 'Cargo.toml']).split(
    '\n',
  );
  const names = [];
  for (const f of files) {
    const s = fs.readFileSync(path.join(repo, f), 'utf8');
    if (!/^version\.workspace\s*=\s*true/m.test(s)) continue;
    const m = /^\[package\][^[]*?^name\s*=\s*"([^"]+)"/ms.exec(s);
    if (m) names.push(m[1]);
  }
  return names.sort();
}

/** The CHANGELOG with "## Unreleased" turned into this release's section (a fresh empty Unreleased above it). */
export function changelogWith(text, to, date, subjects) {
  const head = '# Changelog\n';
  const body = text.startsWith(head) ? text.slice(head.length) : text;
  const m = /^## Unreleased[^\n]*\n([\s\S]*?)(?=^## |(?![\s\S]))/m.exec(body);
  const notes = m ? m[1].trim() : '';
  const lines = notes
    ? notes
    : subjects.length
      ? subjects
          .map((l) => {
            const [h, ...rest] = l.split(' ');
            let s = rest.join(' ');
            if (s.length > 160) s = s.slice(0, 157).trimEnd() + '…';
            return `- ${s} (${h})`;
          })
          .join('\n')
      : '- no user-facing changes; rebuilt';
  const section = `## ${to} (${date})\n\n${lines}\n`;
  const rest = m ? body.replace(m[0], '') : body;
  return `${head}\n## Unreleased\n\n${section}\n${rest.replace(/^\n+/, '')}`.replace(
    /\n{3,}/g,
    '\n\n',
  );
}

/** Apply a plan; returns the files written. */
export function applyBump(
  repo,
  p,
  date = new Date().toISOString().slice(0, 10),
) {
  const v = /("version":\s*")(\d+\.\d+\.\d+)(")/;
  replaceOnce(path.join(repo, CONF), v, p.to);
  for (const f of PACKAGE_JSONS) replaceOnce(path.join(repo, f), v, p.to);
  replaceOnce(
    path.join(repo, 'Cargo.toml'),
    /(\[workspace\.package\][^[]*?^version\s*=\s*")(\d+\.\d+\.\d+)(")/ms,
    p.to,
  );
  const lockFile = path.join(repo, 'Cargo.lock');
  let lock = fs.readFileSync(lockFile, 'utf8');
  for (const n of workspaceMembers(repo)) {
    const re = new RegExp(
      `(\\[\\[package\\]\\]\\nname = "${n.replace(/[-]/g, '\\-')}"\\nversion = ")${p.from.replace(/\./g, '\\.')}(")`,
    );
    lock = lock.replace(re, `$1${p.to}$2`);
  }
  fs.writeFileSync(lockFile, lock);
  const subs = p.since
    ? git(repo, [
        'log',
        '--format=%h %s',
        `${p.since}..HEAD`,
        '--',
        ...SHELL_PATHS,
      ])
        .split('\n')
        .filter(Boolean)
    : [];
  const cl = path.join(repo, 'CHANGELOG.md');
  const old = fs.existsSync(cl) ? fs.readFileSync(cl, 'utf8') : '# Changelog\n';
  fs.writeFileSync(cl, changelogWith(old, p.to, date, subs));
  return [CONF, ...PACKAGE_JSONS, 'Cargo.toml', 'Cargo.lock', 'CHANGELOG.md'];
}

/** Tag HEAD `app-v<version>`; an existing tag must already be at HEAD. Returns the tag made (or null). */
export function tag(repo, push = false) {
  const v = currentVersion(repo);
  const t = `${TAG_PREFIX}${v}`;
  const head = git(repo, ['rev-parse', 'HEAD']);
  let at = '';
  try {
    at = git(repo, ['rev-parse', `${t}^{commit}`]);
  } catch {
    /* new */
  }
  if (at && at !== head)
    throw new Error(
      `${t} exists at ${at.slice(0, 7)}, not HEAD: bump the version`,
    );
  if (!at) git(repo, ['tag', '-a', t, '-m', `qrtz ${v}`]);
  if (push) git(repo, ['push', 'origin', `refs/tags/${t}`]);
  return at ? null : t;
}

function main(argv) {
  const repo = path.resolve(import.meta.dirname, '../..');
  const [cmd, ...rest] = argv;
  const force = rest.includes('--force');
  if (cmd === 'plan' || cmd === 'bump') {
    const p = plan(repo, force);
    if (cmd === 'bump' && p.to)
      for (const f of applyBump(repo, p)) console.log(f);
    console.error(`version: qrtz ${p.from} -> ${p.to ?? '-'} (${p.reason})`);
    if (cmd === 'plan') console.log(`app\t${p.from}\t${p.to ?? '-'}`);
  } else if (cmd === 'tag') {
    const t = tag(repo, rest.includes('--push'));
    if (t) console.log(t);
  } else if (cmd === 'current') {
    console.log(currentVersion(repo));
  } else {
    throw new Error(
      'usage: version.mjs plan|bump [--force] | tag [--push] | current',
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

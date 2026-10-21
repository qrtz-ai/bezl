// QRT-306 (Bozniak): the release-build check for apps/desktop/src-tauri/speech.json, the speech module pins a release
// bakes in (QRTZ_SPEECH_*, apps/desktop/src-tauri/src/backend.rs). Accepted: exactly {"speech":"none"} (0.2.4 shipped
// with no speech module), or exactly the five fields, each valid. Partial, malformed or extra keys fail.
//
//   node scripts/release/check-speech.mjs [speech.json] [--github-env <file>]
//     exit 0 and print "speech: none" or "speech: pinned <url>"; with --github-env and five fields, append the
//     QRTZ_SPEECH_* lines (public values only) so the build bakes them in.
import { createPublicKey } from 'node:crypto';
import { appendFileSync, readFileSync } from 'node:fs';
import path from 'node:path';

const FIELDS = [
  'QRTZ_SPEECH_URL',
  'QRTZ_SPEECH_KEY_ID',
  'QRTZ_SPEECH_KEY',
  'QRTZ_SPEECH_ROOT_KEY_ID',
  'QRTZ_SPEECH_ROOT_KEY',
];
const KEY_ID = /^[A-Za-z0-9._-]{1,64}$/;
const HEX32 = /^[0-9a-f]{64}$/;

function ed25519(hex) {
  if (!HEX32.test(hex)) return false;
  if (/^0+$/.test(hex)) return false;
  try {
    createPublicKey({
      key: Buffer.concat([
        Buffer.from('302a300506032b6570032100', 'hex'),
        Buffer.from(hex, 'hex'),
      ]),
      format: 'der',
      type: 'spki',
    });
    return true;
  } catch {
    return false;
  }
}

/** Returns {mode:'none'} or {mode:'pinned', values}; throws with a reason otherwise. */
export function checkSpeech(text) {
  let v;
  try {
    v = JSON.parse(text);
  } catch {
    throw new Error('speech.json is not json');
  }
  if (!v || typeof v !== 'object' || Array.isArray(v))
    throw new Error('speech.json must be an object');
  const keys = Object.keys(v);
  if (keys.length === 1 && keys[0] === 'speech') {
    if (v.speech !== 'none') throw new Error('"speech" may only be "none"');
    return { mode: 'none' };
  }
  const extra = keys.filter((k) => !FIELDS.includes(k));
  if (extra.length) throw new Error(`unexpected keys: ${extra.join(', ')}`);
  const missing = FIELDS.filter((k) => !(k in v));
  if (missing.length)
    throw new Error(`speech pins partly set; missing ${missing.join(', ')}`);
  for (const k of FIELDS)
    if (typeof v[k] !== 'string') throw new Error(`${k} must be a string`);
  let u;
  try {
    u = new URL(v.QRTZ_SPEECH_URL);
  } catch {
    throw new Error('QRTZ_SPEECH_URL is not a url');
  }
  if (
    u.protocol !== 'https:' ||
    u.username ||
    u.password ||
    u.search ||
    u.hash ||
    /\s/.test(v.QRTZ_SPEECH_URL)
  )
    throw new Error(
      'QRTZ_SPEECH_URL must be a plain https url (no credentials, query or fragment)',
    );
  for (const k of ['QRTZ_SPEECH_KEY_ID', 'QRTZ_SPEECH_ROOT_KEY_ID'])
    if (!KEY_ID.test(v[k]))
      throw new Error(`${k} must be 1-64 of A-Z a-z 0-9 . _ -`);
  for (const k of ['QRTZ_SPEECH_KEY', 'QRTZ_SPEECH_ROOT_KEY'])
    if (!ed25519(v[k]))
      throw new Error(
        `${k} must be a 32-byte Ed25519 public key in lowercase hex`,
      );
  if (v.QRTZ_SPEECH_KEY === v.QRTZ_SPEECH_ROOT_KEY)
    throw new Error('the speech key and root key must differ');
  return { mode: 'pinned', values: v };
}

function main(argv) {
  let file = path.join(
    import.meta.dirname,
    '../../apps/desktop/src-tauri/speech.json',
  );
  let env;
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--github-env') env = argv[++i];
    else file = argv[i];
  }
  let r;
  try {
    r = checkSpeech(readFileSync(file, 'utf8'));
  } catch (e) {
    console.error(`check-speech: FAIL ${file}: ${e.message}`);
    process.exit(1);
  }
  if (r.mode === 'none') {
    console.log('speech: none');
    return;
  }
  if (env)
    appendFileSync(env, FIELDS.map((k) => `${k}=${r.values[k]}\n`).join(''));
  console.log(`speech: pinned ${r.values.QRTZ_SPEECH_URL}`);
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(import.meta.filename)
)
  main(process.argv.slice(2));

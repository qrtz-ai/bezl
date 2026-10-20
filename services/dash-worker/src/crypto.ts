/** HMAC-SHA256 with the server pepper (spec "Tokens hashed with HMAC-SHA256 + server pepper"), and OS randomness. */
const enc = new TextEncoder();
const keys = new Map<string, Promise<CryptoKey>>();

const hexToBytes = (hex: string) => {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++)
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
};
export const toHex = (b: ArrayBuffer | Uint8Array) =>
  [...new Uint8Array(b)].map((x) => x.toString(16).padStart(2, '0')).join('');

/** The pepper is 64 hex characters (32 bytes), the Worker secret DASH_PEPPER. Never logged. */
export const pepperOk = (pepperHex: string | undefined): pepperHex is string =>
  typeof pepperHex === 'string' && /^[0-9a-f]{64}$/i.test(pepperHex);

const key = (pepperHex: string) => {
  let k = keys.get(pepperHex);
  if (!k) {
    k = crypto.subtle.importKey(
      'raw',
      hexToBytes(pepperHex),
      { name: 'HMAC', hash: 'SHA-256' },
      false,
      ['sign'],
    );
    keys.set(pepperHex, k);
  }
  return k;
};

/** hex(HMAC-SHA256(key = pepper bytes, msg)): the Rust `Pepper::hash`. */
export const hmacHex = async (pepperHex: string, msg: string) =>
  toHex(
    await crypto.subtle.sign('HMAC', await key(pepperHex), enc.encode(msg)),
  );

/** hex(HMAC-SHA256(key bytes, msg bytes)) for an arbitrary key: the Rust `hmac_sha256`, for the shared vectors. */
export const hmacRaw = async (keyBytes: Uint8Array, msg: Uint8Array) => {
  const k = await crypto.subtle.importKey(
    'raw',
    keyBytes,
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign'],
  );
  return toHex(await crypto.subtle.sign('HMAC', k, msg));
};

/** Constant-time compare of two equal-length strings (hex hashes). */
export const ctEq = (a: string, b: string) => {
  const x = enc.encode(a);
  const y = enc.encode(b);
  if (x.byteLength !== y.byteLength) return false;
  return crypto.subtle.timingSafeEqual(x, y);
};

export const randomHex = (bytes: number) =>
  toHex(crypto.getRandomValues(new Uint8Array(bytes)));

export const CODE_ALPHABET = '23456789ABCDEFGHJKMNPQRSTVWXYZ';

/** `n` characters of CODE_ALPHABET, uniform: bytes >= 240 are redrawn (rejection sampling, never `% 30` alone). */
export const codeChars = (n: number) => {
  const limit = Math.floor(256 / CODE_ALPHABET.length) * CODE_ALPHABET.length;
  let out = '';
  while (out.length < n) {
    for (const x of crypto.getRandomValues(new Uint8Array(16))) {
      if (x < limit && out.length < n)
        out += CODE_ALPHABET[x % CODE_ALPHABET.length];
    }
  }
  return out;
};

/**
 * QRT-284 (Bozniak): an enrollment code is `<id>.<secret>`. The id (prefix + CODE_ID_CHARS) only finds the row; the
 * secret (CODE_SECRET_CHARS from the CSPRNG, 17 x log2(30) = 83.4 bits) is the part that proves possession. Only
 * HMAC(DASH_ENROLL_KEY, id.secret) is stored, and a code dies after ENROLL_CODE_STRIKES wrong secrets on its id.
 */
export const CODE_ID_CHARS = 8;
export const CODE_SECRET_CHARS = 17;
export const CODE_SECRET_BITS =
  CODE_SECRET_CHARS * Math.log2(CODE_ALPHABET.length);

export const newCode = (prefix = 'ENR-') => {
  const id = `${prefix}${codeChars(CODE_ID_CHARS)}`;
  const secret = codeChars(CODE_SECRET_CHARS);
  return { code: `${id}.${secret}`, id, secret };
};

const CODE_RE = new RegExp(
  `^((?:ENR|TST)-[${CODE_ALPHABET}]{${CODE_ID_CHARS}})\\.([${CODE_ALPHABET}]{${CODE_SECRET_CHARS}})$`,
);

/** Trimmed and upper-cased (hand-typed codes), then split. Null for anything not exactly `<id>.<secret>`. */
export const parseCode = (
  raw: string,
): { id: string; secret: string } | null => {
  const m = CODE_RE.exec(raw.trim().toUpperCase());
  return m ? { id: m[1]!, secret: m[2]! } : null;
};

/** The stored form of a code's secret: hex(HMAC-SHA256(DASH_ENROLL_KEY, "enroll-code:" id "." secret)). */
export const codeMac = (enrollKeyHex: string, id: string, secret: string) =>
  hmacHex(enrollKeyHex, `enroll-code:${id}.${secret}`);

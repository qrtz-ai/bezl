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

export const newCode = () => {
  const c = codeChars(8);
  return `ENR-${c.slice(0, 4)}-${c.slice(4)}`;
};

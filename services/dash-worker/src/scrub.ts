/**
 * The secret scanner (spec "Secret scanner on every payload"), rule for rule the Rust `scrub` in
 * crates/qrtz-dash/src/store.rs after Bozniak's M1. Shared vectors: contracts/dash/vectors.json.
 */
export const SECRET_PREFIXES = [
  'sk-ant-',
  'sk-',
  'sk_live_',
  'rk_live_',
  'ghp_',
  'gho_',
  'ghs_',
  'ghu_',
  'ghr_',
  'github_pat_',
  'xoxb-',
  'xoxp-',
  'xoxa-',
  'xoxs-',
  'xoxr-',
  'xapp-',
  'akia',
  'asia',
  'at_',
  'aiza',
  'glpat-',
  'hf_',
  'npm_',
] as const;

const enc = new TextEncoder();

/** Rust `char::is_whitespace` (Unicode White_Space), which differs from JS `\s` at U+0085 and U+FEFF. */
export const isRustWhitespace = (c: string) => {
  const n = c.codePointAt(0) ?? 0;
  return (
    (n >= 0x09 && n <= 0x0d) ||
    n === 0x20 ||
    n === 0x85 ||
    n === 0xa0 ||
    n === 0x1680 ||
    (n >= 0x2000 && n <= 0x200a) ||
    n === 0x2028 ||
    n === 0x2029 ||
    n === 0x202f ||
    n === 0x205f ||
    n === 0x3000
  );
};

const SPLIT = new Set([
  '=',
  ':',
  '"',
  "'",
  ',',
  ';',
  '(',
  ')',
  '[',
  ']',
  '{',
  '}',
  '<',
  '>',
]);
const isSplit = (c: string) => isRustWhitespace(c) || SPLIT.has(c);

const entropy = (s: string) => {
  const counts = new Map<number, number>();
  const b = enc.encode(s);
  for (const x of b) counts.set(x, (counts.get(x) ?? 0) + 1);
  let h = 0;
  for (const c of counts.values()) {
    const p = c / b.length;
    h -= p * Math.log2(p);
  }
  return h;
};

const B64URL = /^[A-Za-z0-9_-]+$/;
const B64 = /^[A-Za-z0-9+/_-]+$/;
const isJwt = (w: string) => {
  const p = w.split('.');
  return (
    p.length === 3 &&
    w.startsWith('eyJ') &&
    p[0]!.length >= 10 &&
    p[1]!.length >= 10 &&
    p.every((x) => x.length > 0 && B64URL.test(x))
  );
};

const trimSet = (s: string, set: string) => {
  let a = 0;
  let b = s.length;
  while (a < b && set.includes(s[a]!)) a++;
  while (b > a && set.includes(s[b - 1]!)) b--;
  return s.slice(a, b);
};

/** ASCII-only lowercasing: one code unit in, one out, so offsets never shift (Bozniak M; Rust `to_ascii_lowercase`). */
const asciiLower = (s: string) =>
  s.replace(/[A-Z]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 32));

const ZERO_WIDTH = /[\u200B-\u200D\u2060\uFEFF]/g;
const isAsciiAlnum = (c: string | undefined) =>
  c !== undefined && /^[a-z0-9]$/i.test(c);

/**
 * A key prefix at the word start or after any character that isn't an ASCII letter or digit (Bozniak M), checked
 * with zero-width characters removed and NFKC applied (Bozniak L). Rust `has_secret_prefix`.
 */
const hasSecretPrefix = (w: string) => {
  const lw = asciiLower(w.replace(ZERO_WIDTH, '').normalize('NFKC'));
  for (let i = 0; i < lw.length; i++) {
    if (i > 0 && isAsciiAlnum(lw[i - 1])) continue;
    const tok = /^[a-z0-9_-]*/.exec(lw.slice(i))![0];
    if (
      SECRET_PREFIXES.some(
        (p) => tok.startsWith(p) && tok.length >= p.length + 12,
      )
    )
      return true;
  }
  return false;
};

/** camelCase / ALLCAPS / digit runs average 4+ characters: an identifier (Bozniak H2). Rust `is_wordy`. */
const isWordy = (w: string) => {
  const t = w.match(/[A-Z]?[a-z]+|[A-Z]+(?![a-z])|[0-9]+/g) ?? [];
  return t.length > 0 && t.join('').length / t.length >= 4;
};

/** One _ / - segment of an identifier: wordy, 3 characters or fewer, all digits, or one case class of at most 7
 * characters (Bozniak H, review of ded56dd5). Rust `is_ident_segment`. */
const isIdentSegment = (p: string) =>
  isWordy(p) ||
  p.length <= 3 ||
  /^[0-9]+$/.test(p) ||
  /^(?:[A-Z0-9]{1,7}|[a-z]{1,7})$/.test(p);

/** Wordy as a whole, or every _ / - segment an identifier segment (Bozniak L). Rust `is_identifier`. */
const isIdentifier = (w: string) =>
  isWordy(w) || (/[_-]/.test(w) && w.split(/[_-]/).every(isIdentSegment));

/** The entropy and JWT checks on one candidate, trimmed like a word, plus the separator-free form of a word
 * with no / that has _ or - (Bozniak, review of ce60c75f). Rust `is_dense`. */
const isDense = (s: string) => {
  const c = trimSet(s, '.!?`*');
  return isHighEntropy(c) || isJwt(c) || isDenseJoined(c);
};

/** c without _ / -, entropy-checked when c has no /, had a separator and is still 24+ characters. Rust `is_dense_joined`. */
const isDenseJoined = (c: string) => {
  if (c.includes('/') || !/[_-]/.test(c)) return false;
  const j = c.replace(/[_-]/g, '');
  return j.length >= 24 && isHighEntropy(j);
};

/** Bozniak's H2 rule (his prototype `rule`). Rust `is_high_entropy`. */
const isHighEntropy = (core: string) => {
  const w = core.replace(/=+$/, '');
  if (w.length < 24 || !B64.test(w)) return false;
  const classes = [/[a-z]/, /[A-Z]/, /[0-9]/].filter((r) => r.test(w)).length;
  if (classes < 2 || /^[0-9a-fA-F]+$/.test(w)) return false;
  const pathOk =
    w.includes('/') &&
    w
      .split('/')
      .every(
        (p) => /^[A-Za-z0-9_.-]{0,24}$/.test(p) && (isWordy(p) || p.length < 8),
      );
  if (pathOk || isIdentifier(w)) return false;
  return entropy(w) >= 0.8 * Math.log2(Math.min(w.length, 64));
};

const isSecretWord = (w: string, shaCtx: boolean) => {
  const core = trimSet(w, '.!?`*');
  const keyish = hasSecretPrefix(w);
  const ssn = /^\d{3}-\d{2}-\d{4}$/.test(core);
  // Exactly 40 hex is a git object hash; 64 hex after sha256: / sha256= is a digest (Bozniak L3, H2).
  const longHex =
    core.length >= 32 &&
    core.length !== 40 &&
    !(core.length === 64 && shaCtx) &&
    /^[0-9A-Fa-f]+$/.test(core);
  // One non-ASCII character can't hide a secret (Bozniak M): also check the NFKC form without zero-width characters,
  // that form with non-ASCII removed, and every ASCII run of 24+ characters. Rust `is_secret_word`.
  const norm = w.replace(ZERO_WIDTH, '').normalize('NFKC');
  const ascii = norm.replace(/[\u0080-\uFFFF]/g, '');
  const runs = [w, norm].flatMap((x) =>
    x.split(/[\u0080-\uFFFF]+/).filter((r) => r.length >= 24),
  );
  return keyish || ssn || longHex || [core, norm, ascii, ...runs].some(isDense);
};

const scrubPem = (s: string): [string, boolean] => {
  let hit = false;
  for (;;) {
    // Case-insensitive, ASCII only so offsets match the original (Bozniak L2, M).
    const start = asciiLower(s).indexOf('-----begin');
    if (start < 0) return [s, hit];
    const rest = asciiLower(s.slice(start));
    let end = rest.length;
    const e = rest.indexOf('-----end');
    if (e >= 0) {
      const d = rest.slice(e + 8).indexOf('-----');
      end = d < 0 ? rest.length : e + 8 + d + 5;
    }
    s = s.slice(0, start) + '[redacted]' + s.slice(start + end);
    hit = true;
  }
};

const luhn = (d: number[]) =>
  d
    .slice()
    .reverse()
    .reduce((sum, x, i) => {
      if (i % 2 === 1) {
        const y = x * 2;
        return sum + (y > 9 ? y - 9 : y);
      }
      return sum + x;
    }, 0) %
    10 ===
  0;

const isDigit = (c: string | undefined) =>
  c !== undefined && c >= '0' && c <= '9';

/** Redact PEM blocks, key/token shapes, JWTs, high-entropy strings, SSNs and Luhn-valid card numbers. */
export const scrub = (input: string): { text: string; hit: boolean } => {
  let [s, hit] = scrubPem(input);
  let out = '';
  let word = '';
  const flush = () => {
    if (word) {
      const shaCtx = /sha256[:=]$/i.test(out.slice(-7));
      if (isSecretWord(word, shaCtx)) {
        out += '[redacted]';
        hit = true;
      } else out += word;
      word = '';
    }
  };
  for (const c of s) {
    if (isSplit(c)) {
      flush();
      out += c;
    } else word += c;
  }
  flush();
  s = out;
  const chars = [...s];
  out = '';
  let i = 0;
  while (i < chars.length) {
    if (isDigit(chars[i])) {
      let j = i;
      const digits: number[] = [];
      while (
        j < chars.length &&
        (isDigit(chars[j]) ||
          ((chars[j] === ' ' || chars[j] === '-') && isDigit(chars[j + 1])))
      ) {
        if (isDigit(chars[j])) digits.push(Number(chars[j]));
        j++;
      }
      if (digits.length >= 13 && digits.length <= 19 && luhn(digits)) {
        out += '[redacted]';
        hit = true;
      } else out += chars.slice(i, j).join('');
      i = j;
    } else {
      out += chars[i];
      i++;
    }
  }
  return { text: out, hit };
};

/** Scrub every string in a payload, object keys included (keys in sorted order, like serde_json's map). */
export const scrubValue = (v: unknown): { value: unknown; hit: boolean } => {
  if (typeof v === 'string') {
    const r = scrub(v);
    return { value: r.text, hit: r.hit };
  }
  if (Array.isArray(v)) {
    let hit = false;
    const value = v.map((x) => {
      const r = scrubValue(x);
      hit ||= r.hit;
      return r.value;
    });
    return { value, hit };
  }
  if (v && typeof v === 'object') {
    let hit = false;
    const value: Record<string, unknown> = {};
    for (const k of Object.keys(v).sort()) {
      const rk = scrub(k);
      const rv = scrubValue((v as Record<string, unknown>)[k]);
      hit ||= rk.hit || rv.hit;
      value[rk.text] = rv.value;
    }
    return { value, hit };
  }
  return { value: v, hit: false };
};

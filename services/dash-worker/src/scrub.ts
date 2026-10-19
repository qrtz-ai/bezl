/**
 * The secret scanner (spec "Secret scanner on every payload"), rule for rule the Rust `scrub` in
 * crates/qrtz-dash/src/store.rs after Bozniak's M1. Shared vectors: contracts/dash/scrub-vectors.json.
 */
export const SECRET_PREFIXES = [
  "sk-ant-",
  "sk-",
  "sk_live_",
  "rk_live_",
  "ghp_",
  "gho_",
  "ghs_",
  "ghu_",
  "ghr_",
  "github_pat_",
  "xoxb-",
  "xoxp-",
  "xoxa-",
  "xoxs-",
  "xoxr-",
  "xapp-",
  "akia",
  "asia",
  "at_",
  "aiza",
  "glpat-",
  "hf_",
  "npm_",
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
  "=",
  ":",
  '"',
  "'",
  ",",
  ";",
  "(",
  ")",
  "[",
  "]",
  "{",
  "}",
  "<",
  ">",
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
  const p = w.split(".");
  return (
    p.length === 3 &&
    w.startsWith("eyJ") &&
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

/** A key prefix at the word start or after any non-alphanumeric character (Bozniak M; Rust `has_secret_prefix`). */
const hasSecretPrefix = (lw: string) => {
  for (let i = 0; i < lw.length; i++) {
    if (i > 0 && /[a-z0-9]/.test(lw[i - 1]!)) continue;
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

const isSecretWord = (w: string) => {
  const core = trimSet(w, ".!?`*");
  const lw = core.replace(/[A-Z]/g, (c) => c.toLowerCase());
  const keyish = hasSecretPrefix(lw);
  const ssn = /^\d{3}-\d{2}-\d{4}$/.test(core);
  // A git object hash (exactly 40 hex characters) is not a secret (Bozniak L3).
  const longHex =
    core.length >= 32 && core.length !== 40 && /^[0-9A-Fa-f]+$/.test(core);
  // Needs a digit, upper and lower case and no run of 4+ lowercase letters (Bozniak L3).
  const b64 = core.replace(/=+$/, "");
  const highEntropy =
    b64.length >= 32 &&
    B64.test(b64) &&
    /[0-9]/.test(b64) &&
    /[A-Z]/.test(b64) &&
    /[a-z]/.test(b64) &&
    !/[a-z]{4}/.test(b64) &&
    entropy(b64) >= 4.0;
  return keyish || ssn || longHex || highEntropy || isJwt(core);
};

const scrubPem = (s: string): [string, boolean] => {
  let hit = false;
  for (;;) {
    // Case-insensitive (Bozniak L2).
    const start = s.toLowerCase().indexOf("-----begin");
    if (start < 0) return [s, hit];
    const rest = s.slice(start).toLowerCase();
    let end = rest.length;
    const e = rest.indexOf("-----end");
    if (e >= 0) {
      const d = rest.slice(e + 8).indexOf("-----");
      end = d < 0 ? rest.length : e + 8 + d + 5;
    }
    s = s.slice(0, start) + "[redacted]" + s.slice(start + end);
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
  c !== undefined && c >= "0" && c <= "9";

/** Redact PEM blocks, key/token shapes, JWTs, high-entropy strings, SSNs and Luhn-valid card numbers. */
export const scrub = (input: string): { text: string; hit: boolean } => {
  let [s, hit] = scrubPem(input);
  let out = "";
  let word = "";
  const flush = () => {
    if (word) {
      if (isSecretWord(word)) {
        out += "[redacted]";
        hit = true;
      } else out += word;
      word = "";
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
  out = "";
  let i = 0;
  while (i < chars.length) {
    if (isDigit(chars[i])) {
      let j = i;
      const digits: number[] = [];
      while (
        j < chars.length &&
        (isDigit(chars[j]) ||
          ((chars[j] === " " || chars[j] === "-") && isDigit(chars[j + 1])))
      ) {
        if (isDigit(chars[j])) digits.push(Number(chars[j]));
        j++;
      }
      if (digits.length >= 13 && digits.length <= 19 && luhn(digits)) {
        out += "[redacted]";
        hit = true;
      } else out += chars.slice(i, j).join("");
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
  if (typeof v === "string") {
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
  if (v && typeof v === "object") {
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

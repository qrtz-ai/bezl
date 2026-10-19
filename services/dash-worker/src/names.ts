/** Agent names and widget keys: the Rust `clean_name`, `skeleton`, `RESERVED` and `valid_key` in store.rs. */
import { isRustWhitespace } from './scrub';

export const RESERVED = [
  'security',
  'system',
  'admin',
  'agentdash',
  'qrtz',
  'dash',
  'support',
  'you',
];

const isBad = (c: string) => {
  const n = c.codePointAt(0) ?? 0;
  return (
    n <= 0x1f ||
    (n >= 0x7f && n <= 0x9f) ||
    n === 0x00ad ||
    n === 0x061c ||
    n === 0x180e ||
    (n >= 0x200b && n <= 0x200f) ||
    (n >= 0x202a && n <= 0x202e) ||
    (n >= 0x2060 && n <= 0x206f) ||
    n === 0xfeff ||
    (n >= 0xfff9 && n <= 0xfffb)
  );
};

/** Strip format/bidi/control characters, collapse whitespace. */
export const cleanName = (s: string) => {
  const kept = [...s].filter((c) => !isBad(c)).join('');
  const words: string[] = [];
  let w = '';
  for (const c of kept) {
    if (isRustWhitespace(c)) {
      if (w) words.push(w);
      w = '';
    } else w += c;
  }
  if (w) words.push(w);
  return words.join(' ');
};

const FOLD: Record<string, string> = {
  '0': 'o',
  о: 'o',
  ο: 'o',
  '1': 'i',
  l: 'i',
  і: 'i',
  ı: 'i',
  '|': 'i',
  а: 'a',
  α: 'a',
  е: 'e',
  ε: 'e',
  с: 'c',
  р: 'p',
  ρ: 'p',
  х: 'x',
  у: 'y',
  ѕ: 's',
  '5': 's',
};

/** The crude confusable skeleton (ADR 0228 stub for UTS #39). */
export const skeleton = (s: string) =>
  [...s]
    .flatMap((c) => [...c.toLowerCase()])
    .map((c) => FOLD[c] ?? (/^[a-z0-9]$/.test(c) ? c : ''))
    .join('')
    .replaceAll('rn', 'm');

export const validKey = (k: string) =>
  k.length > 0 &&
  k.length <= 64 &&
  !k.startsWith('-') &&
  !k.endsWith('-') &&
  !k.includes('--') &&
  /^[a-z0-9-]+$/.test(k);

export const charCount = (s: string) => [...s].length;

/**
 * Scripts written without spaces between words (Thai, Lao, Myanmar, Khmer, kana, Han). unicode61 indexes a run of
 * them as one token, so only a word at the start of a run is found by the index (N48). Fixed code point ranges, not
 * `\p{Script}`, so TS and Rust agree whatever Unicode version each ships. Same class as Rust `UNSEGMENTED`.
 */
export const UNSEGMENTED =
  /[\u0E00-\u0EFF\u1000-\u109F\u1780-\u17FF\u3005-\u3007\u3040-\u30FF\u31F0-\u31FF\u3400-\u4DBF\u4E00-\u9FFF\uF900-\uFAFF\u{20000}-\u{3134F}]/u;

/** At most this many bigrams per chat question (each one is a substring test per line in the scan). */
export const CHAT_MAX_BIGRAMS = 16;

/**
 * Character bigrams of a word from an unsegmented script (N49, ADR 0047), in order, by code point. A two-character
 * word is its own bigram. Chinese and Japanese words are mostly two characters, so a bigram of the question that a
 * line contains is a good sign the line is about it, without a dictionary.
 */
export function bigrams(word: string): string[] {
  const cs = Array.from(word);
  if (cs.length <= 2) return [word];
  const out: string[] = [];
  for (let i = 0; i + 1 < cs.length; i++) out.push(cs[i]! + cs[i + 1]!);
  return out;
}

import type { DictionaryEntry } from './contract';

const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
// ADR 0101: the stutter-prone words, the only ones whose immediate repeat collapses ("I I think", "the the").
// Everything else is kept as said: numbers ("10 10", "1 1 2 3"), grammatical doubles ("had had", "that that",
// "is is", "what it was was", "her her", "come in in the morning", "turn it on on Friday", "Will will"), and
// emphasis or idiom ("very very", "no no", "bye bye", "so so", "my my", "there there").
const STUTTERS = new Set([
  'i',
  'a',
  'an',
  'the',
  'to',
  'and',
  'but',
  'or',
  'of',
  'at',
  'for',
  'with',
  'from',
  'we',
  'you',
  'he',
  'she',
  'it',
  'they',
  'me',
  'our',
  'your',
  'his',
  'their',
  'this',
  'be',
  'would',
  'can',
  'could',
  'should',
  'did',
  'have',
  'has',
  'if',
  'when',
  'what',
  'just',
]);
// Repeats across a comma ("I love you, you know", "take it, it is yours") are a clause boundary as often as a
// stutter; only these collapse there ("I, I think", "the, the plan").
const COMMA_STUTTERS = new Set(['i', 'a', 'an', 'the']);
// An immediate repeat of a whole word; the apostrophe is part of a word on both sides ("you, you're" is no repeat).
const REPEATS = /(?<![\w'])(\w+)(?:[\s,]+\1(?![\w']))+/gi;

/** ADR 0101: up to this many words is a short dictation (a reply, a search, a field): no "." is added, and a lone
 * "." the recogniser ended it with is dropped ("Yes", "Sounds good", "Friday at 3"). "?" and "!" stay. */
export const SHORT_DICTATION_WORDS = 3;

function terminalPunctuation(t: string): string {
  const words = t.split(/\s+/).filter(Boolean);
  const short =
    !t.includes('\n') &&
    words.length <= SHORT_DICTATION_WORDS &&
    !/[.!?]\s/.test(t);
  if (short) {
    // A lone final "." only: not "...", not the last dot of "p.m." or "U.S.".
    const last = words.at(-1) ?? '';
    const body = last.endsWith('.') ? last.slice(0, -1) : null;
    return body && !body.includes('.') ? t.slice(0, -1) : t;
  }
  return /[\p{L}\p{N}]$/u.test(t) ? `${t}.` : t;
}

// ADR 0120 (N167): a lone "." on a word with a lowercase word after it is not a sentence end (the recogniser
// lowercased what follows; on a segmented recogniser, ADR 0118, it is the "." each segment's decode writes at its cut).
// The dot is dropped unless the word is an abbreviation; either way the next word stays lowercase. Only when the raw
// text has a capital letter somewhere (the recogniser writes case).
const CONTINUED_DOT = /(?<![\w.])([A-Za-z]+)\.(?=\s+[a-z])/g;
/** ADR 0120: an abbreviation's "." is never a sentence end before a lowercase word. Same list in cleanup.rs. */
export const ABBREVIATIONS = new Set([
  'mr',
  'mrs',
  'ms',
  'dr',
  'prof',
  'sr',
  'jr',
  'st',
  'vs',
  'etc',
  'approx',
  'inc',
  'ltd',
  'corp',
  'mt',
  'dept',
  'ave',
  'blvd',
]);
const isAbbreviation = (w: string) => ABBREVIATIONS.has(w.toLowerCase());
/** Does `before` (the text up to a ". ") end with a whole abbreviation word ("... Mr", not "... Amr")? */
const endsWithAbbreviation = (before: string) => {
  const m = /(?<![\w.])([A-Za-z]+)$/.exec(before);
  return m !== null && isAbbreviation(m[1]);
};

const FILLERS =
  /(^|[\s,])(?:u+m+|u+h+|e+r+m+|e+r|a+h+|h+m+|m+h*m+)(?=[\s,.!?]|$)[,]?/gi;

/** Writes dictionary terms the way the user spells them ("cue arts" → "qrtz" when listed as an alias). */
export function applyDictionary(
  text: string,
  dict: readonly DictionaryEntry[],
): string {
  let out = text;
  for (const d of dict) {
    for (const said of [d.term, ...(d.aliases ?? [])]) {
      const words = said.trim().split(/\s+/).map(esc).join('[\\s-]+');
      if (!words) continue;
      out = out.replace(
        new RegExp(`(?<![\\w])${words}(?![\\w])`, 'gi'),
        d.term,
      );
    }
  }
  return out;
}

/**
 * On-device cleanup: spoken commands, fillers, stutters, spacing, capitalization, terminal punctuation (ADR 0101:
 * which repeats collapse, and no "." on a short dictation).
 * Deterministic and instant; talk's only cleanup (no LLM pass, ADR 0071).
 */
export function ruleCleanup(
  raw: string,
  dict: readonly DictionaryEntry[] = [],
): string {
  let t = ` ${raw} `;
  t = t
    .replace(/\s*\bnew paragraph\b[,.]?\s*/gi, '\n\n')
    .replace(/\s*\bnew line\b[,.]?\s*/gi, '\n');
  t = t.replace(FILLERS, '$1');
  // Immediate repeats of stutter-prone words ("I I think", "the the"); ADR 0101.
  t = t.replace(REPEATS, (all: string, w: string) =>
    (all.includes(',') ? COMMA_STUTTERS : STUTTERS).has(w.toLowerCase())
      ? w
      : all,
  );
  t = t.replace(/[ \t]+/g, ' ').replace(/ *\n */g, '\n');
  t = t
    .replace(/\s+([,.!?;:])/g, '$1')
    // ADR 0101 (N160): only "," and ";" get a space after them ("qrtz.ai", "p.m.", "https://" stay as written).
    .replace(/([,;])(?=[^\s\d"')\]])/g, '$1 ');
  t = t.replace(/,+/g, ',').replace(/^[\s,]+|[\s,]+$/g, '');
  // Only for a recogniser that writes capitals: an all-lowercase text's "so. i can" is its own sentence end.
  if (/[A-Z]/.test(raw))
    t = t.replace(CONTINUED_DOT, (all: string, w: string) =>
      isAbbreviation(w) ? all : w,
    );
  t = t.replace(
    // Not after a dotted abbreviation ("e.g. the", "3 p.m. today"), nor after "Mr." / "etc." (ADR 0120).
    /(^|(?<![A-Za-z]\.[A-Za-z])[.!?]\s+|\n)([a-z])/g,
    (all: string, p: string, c: string, at: number, src: string) =>
      p.startsWith('.') && endsWithAbbreviation(src.slice(0, at))
        ? all
        : p + c.toUpperCase(),
  );
  // "i" before a dot only at a sentence end, not the "i" of "i.e." (ADR 0101, N160).
  t = t.replace(/\bi\b(?=[\s,!?']|\.(?![A-Za-z])|$)/g, 'I');
  t = applyDictionary(t, dict);
  return terminalPunctuation(t);
}

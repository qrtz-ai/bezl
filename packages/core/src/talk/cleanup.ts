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

/** ADR 0188: a list of short lines ("Groceries⏎Eggs⏎Milk", an address, a sign-off): at least this many lines. */
export const SHORT_LIST_LINES = 3;

/** ADR 0188: at least SHORT_LIST_LINES lines, no blank line, each 1 to SHORT_DICTATION_WORDS words with no sentence
 * mark inside it, none but the last ending in ".", "!" or "?". Same rule in cleanup.rs. */
function isShortList(t: string): boolean {
  if (t.includes('\n\n')) return false;
  const lines = t.split('\n');
  if (lines.length < SHORT_LIST_LINES) return false;
  return lines.every((l, i) => {
    const n = l.split(/\s+/).filter(Boolean).length;
    return (
      n >= 1 &&
      n <= SHORT_DICTATION_WORDS &&
      !/[.!?]\s/.test(l) &&
      (i === lines.length - 1 || !/[.!?]$/.test(l))
    );
  });
}

function terminalPunctuation(t: string): string {
  // ADR 0188: a list of short lines ends like a short dictation; a recogniser's "." goes only at a clean end.
  if (isShortList(t))
    return t.endsWith('.') && cleanSentenceEnd(t) ? t.slice(0, -1) : t;
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

// The filler word is group 2; whether it goes is keepsFiller's call (ADR 0182).
const FILLERS =
  /(^|[\s,])(u+m+|u+h+|e+r+m+|e+r|a+h+|h+m+|m+h*m+)(?=[\s,.!?]|$)[,]?/gi;

/**
 * ADR 0182 (N185): a filler-shaped word that is a real word here, so it stays. Only unambiguous forms: written in
 * capitals ("the ER", "UM beat Ohio State": the recogniser never writes a filler in capitals), "mm" right after a
 * number ("a 35 mm lens"). ("err" never matches: `e+r` is one "r".)
 * Same rule in cleanup.rs.
 */
function keepsFiller(word: string, all: string, before: string): boolean {
  if (word.length >= 2 && /^[A-Z]+$/.test(word)) return true;
  return (
    word.toLowerCase() === 'mm' &&
    all.startsWith(' ') &&
    !all.endsWith(',') &&
    /[0-9]$/.test(before)
  );
}

/** ADR 0182 (N186): "new line" / "new paragraph" after one of these is a noun phrase ("a new line of products"),
 * never the command, so the words stay. Only words that can't end a clause ("call her new line then ..." is the
 * command). Same list in cleanup.rs. */
export const COMMAND_DETERMINERS = new Set([
  'a',
  'an',
  'the',
  'my',
  'your',
  'our',
  'their',
  'its',
  'every',
]);
const layoutCommand =
  (withText: string) =>
  (all: string, at: number, src: string): string => {
    const m = /(?<![\w'])([A-Za-z]+)$/.exec(src.slice(0, at).trimEnd());
    return m !== null && COMMAND_DETERMINERS.has(m[1].toLowerCase())
      ? all
      : withText;
  };

/** ADR 0186: words that, right after "Scratch that," or "Scratch that.", make it a literal request ("Scratch that,
 * please."), not the correction. Same list in cleanup.rs. */
export const SCRATCH_LITERAL_NEXT = new Set([
  'please',
  'would',
  'will',
  'can',
  'could',
  'and',
  'or',
  'but',
  'too',
  'again',
  'now',
  'for',
  'gently',
  'here',
  'there',
  'okay',
  'ok',
]);

const asciiLower = (s: string) => s.replace(/[A-Z]/g, (c) => c.toLowerCase());

/** Is the ".", "!" or "?" that ends `p` an unambiguous sentence end? The word before it is ASCII letters or digits
 * (an apostrophe inside is fine), at least two, with no dot of its own. Same rule in cleanup.rs. */
function cleanSentenceEnd(p: string): boolean {
  const mark = p.at(-1);
  if (mark !== '.' && mark !== '!' && mark !== '?') return false;
  const body = p.slice(0, -1);
  const token = body.split(/[ \n]/).at(-1) ?? '';
  if (token.length < 2 || !/^[A-Za-z0-9']+$/.test(token)) return false;
  if (!/[A-Za-z0-9]$/.test(token)) return false;
  return !(mark === '.' && endsWithAbbreviation(body));
}

/** The text with the "Scratch that" at `at` applied, or null to leave it as spoken (ADR 0186). */
function scratchAt(t: string, at: number): string | null {
  const before = t.slice(0, at);
  const p = before.replace(/ +$/, '');
  if (p.length === before.length || !cleanSentenceEnd(p)) return null;
  let after = t.slice(at + 'scratch that'.length);
  if (!after.startsWith(',') && !after.startsWith('.')) return null;
  after = after.slice(1);
  const rest = after.replace(/^ +/, '');
  if (rest.length === after.length || !/^[A-Za-z0-9]/.test(rest)) return null;
  const next = asciiLower(/^[A-Za-z0-9]*/.exec(rest)?.[0] ?? '');
  if (SCRATCH_LITERAL_NEXT.has(next)) return null;
  const body = p.slice(0, -1);
  let start = 0;
  for (let j = body.length; j >= 1; j--) {
    const c = body[j - 1];
    if (c === '\n') {
      start = j;
      break;
    }
    if ((c === '.' || c === '!' || c === '?') && body[j] === ' ') {
      if (!cleanSentenceEnd(body.slice(0, j))) return null;
      start =
        j + (body.slice(j).length - body.slice(j).replace(/^ +/, '').length);
      break;
    }
  }
  return t.slice(0, start) + rest;
}

/**
 * ADR 0186: the explicit self-correction "Scratch that", only as its own sentence after a clean sentence end and
 * followed by "," or "." and the replacing words: the sentence before it goes, with the command. Anything unclear is
 * left as spoken. Same rule in cleanup.rs.
 */
function scratchThat(t: string): string {
  const CMD = 'scratch that';
  // One lowercase copy, redone only after a rewrite (rare).
  let lower = asciiLower(t);
  let from = 0;
  for (let at = lower.indexOf(CMD); at >= 0; at = lower.indexOf(CMD, from)) {
    from = at + CMD.length;
    const out = scratchAt(t, at);
    if (out !== null) {
      t = out;
      lower = asciiLower(t);
      from = 0;
    }
  }
  return t;
}

/** ADR 0189: the spoken marks a dictation can end with, and what they become. */
const FINAL_MARKS: readonly (readonly [string, string])[] = [
  ['question mark', '?'],
  ['exclamation point', '!'],
  ['exclamation mark', '!'],
];
/** ADR 0189: a sentence ending in a spoken "question mark" must start like a question. Same list in cleanup.rs. */
export const QUESTION_STARTS = new Set([
  'is',
  'are',
  'was',
  'were',
  'am',
  'do',
  'does',
  'did',
  'can',
  'could',
  'will',
  'would',
  'should',
  'shall',
  'may',
  'might',
  'must',
  'have',
  'has',
  'had',
  'what',
  "what's",
  'when',
  "when's",
  'where',
  "where's",
  'why',
  'who',
  "who's",
  'whom',
  'whose',
  'which',
  'how',
  "how's",
  "isn't",
  "aren't",
  "wasn't",
  "weren't",
  "don't",
  "doesn't",
  "didn't",
  "can't",
  "couldn't",
  "won't",
  "wouldn't",
  "shouldn't",
  "haven't",
  "hasn't",
]);
/** ADR 0189: right before a spoken mark, these make it the noun. Same list in cleanup.rs. */
export const MARK_NOUN_BEFORE = new Set([
  // determiners, demonstratives, possessives, quantifiers
  'a',
  'an',
  'the',
  'this',
  'that',
  'these',
  'those',
  'my',
  'your',
  'his',
  'its',
  'our',
  'their',
  'no',
  'another',
  'any',
  'each',
  'every',
  'some',
  'one',
  'which',
  'whose',
  'what',
  'same',
  'single',
  'extra',
  'double',
  'final',
  'last',
  'first',
  'big',
  'little',
  'small',
  'huge',
  'giant',
  'lone',
  'stray',
  'missing',
  'trailing',
  'closing',
  'inverted',
  // prepositions and conjunctions
  'with',
  'without',
  'of',
  'for',
  'to',
  'from',
  'by',
  'after',
  'before',
  'than',
  'like',
  'as',
  'about',
  'into',
  'onto',
  'at',
  'plus',
  'minus',
  'and',
  'or',
  'nor',
  'versus',
  'vs',
  'via',
  'per',
  'between',
  // copulas and naming
  'is',
  'are',
  'was',
  'were',
  'be',
  'been',
  'being',
  "isn't",
  "wasn't",
  "it's",
  "that's",
  "what's",
  "there's",
  "here's",
  'become',
  'becomes',
  'became',
  'call',
  'called',
  'name',
  'named',
  'label',
  'labeled',
  'labelled',
  'mean',
  'means',
  'meant',
  'spell',
  'spelled',
  // verbs that take the symbol as an object
  'say',
  'says',
  'said',
  'type',
  'types',
  'typed',
  'typing',
  'press',
  'presses',
  'pressed',
  'pressing',
  'hit',
  'hits',
  'put',
  'puts',
  'putting',
  'add',
  'adds',
  'added',
  'adding',
  'use',
  'uses',
  'used',
  'using',
  'insert',
  'inserts',
  'inserted',
  'remove',
  'removes',
  'removed',
  'delete',
  'deletes',
  'deleted',
  'need',
  'needs',
  'needed',
  'want',
  'wants',
  'wanted',
  'have',
  'has',
  'had',
  'having',
  'get',
  'gets',
  'got',
  'give',
  'gives',
  'gave',
  'make',
  'makes',
  'made',
  'include',
  'includes',
  'included',
  'keep',
  'keeps',
  'kept',
  'drop',
  'drops',
  'dropped',
  'lose',
  'loses',
  'lost',
  'miss',
  'misses',
  'missed',
  'see',
  'sees',
  'saw',
  'seen',
  'show',
  'shows',
  'showed',
  'showing',
  'print',
  'prints',
  'printed',
  'display',
  'displays',
  'return',
  'returns',
  'returned',
  'write',
  'writes',
  'wrote',
  'end',
  'ends',
  'ended',
  'ending',
  'likes',
  'love',
  'loves',
  'hate',
  'hates',
  'prefer',
  'prefers',
  'try',
  'tried',
  'place',
  'append',
  'follow',
  'follows',
  'followed',
  'deserve',
  'deserves',
  'deserved',
  'forget',
  'forgets',
  'forgot',
  'leave',
  'left',
  'change',
  'changed',
  'replace',
  'replaced',
  'find',
  'found',
  'click',
  'tap',
  'select',
  'produce',
  'produces',
  'draw',
  'drew',
  'read',
  'reads',
  'count',
  'overuse',
  'overuses',
]);
/** ADR 0189: "there" / "here" right after one of these is "is there [a] question mark". */
const COPULAS = new Set([
  'is',
  'are',
  'was',
  'were',
  "isn't",
  "aren't",
  "wasn't",
  "weren't",
]);
/** ADR 0189: an article, possessive or quantifier in the last MARK_DETERMINER_WINDOW words makes it the noun. */
const MARK_DETERMINER_WINDOW = 3;
const MARK_DETERMINERS = new Set([
  'a',
  'an',
  'the',
  'my',
  'your',
  'his',
  'its',
  'our',
  'their',
  'no',
  'another',
  'every',
  'each',
  'any',
  'some',
  'one',
]);

/**
 * ADR 0189: a spoken "question mark" / "exclamation point" / "exclamation mark" that ends the dictation becomes "?" /
 * "!", only in lowercase, as the very last words, after one space and a plain word that is not in MARK_NOUN_BEFORE,
 * with no MARK_DETERMINERS word in the last MARK_DETERMINER_WINDOW words, and for "question mark" a sentence that
 * starts like a question. Anything else is left as spoken. Same rule in cleanup.rs.
 */
function finalMark(t: string): string {
  const s = /[.?!]$/.test(t) ? t.slice(0, -1) : t;
  for (const [phrase, mark] of FINAL_MARKS) {
    if (!s.endsWith(phrase)) continue;
    const before = s.slice(0, -phrase.length);
    if (!before.endsWith(' ')) return t;
    const body = before.slice(0, -1);
    if (!/[A-Za-z0-9]$/.test(body)) return t;
    let start = 0;
    for (let j = body.length - 1; j >= 1; j--) {
      const c = body[j - 1];
      if (
        c === '\n' ||
        ((c === '.' || c === '!' || c === '?') && body[j] === ' ')
      ) {
        start = j;
        break;
      }
    }
    const words = body
      .slice(start)
      .split(/\s+/)
      .filter(Boolean)
      .map((w) => asciiLower(w.replace(/,+$/, '')));
    const last = words.at(-1);
    if (last === undefined) return t;
    // A plain word: not a number ("Shift 1 exclamation point", "error 404 question mark" name a key or code).
    if (
      !/^[a-z0-9']+$/.test(last) ||
      /^[0-9]+$/.test(last) ||
      MARK_NOUN_BEFORE.has(last)
    )
      return t;
    const prev = words.at(-2);
    if (
      (last === 'there' || last === 'here') &&
      prev !== undefined &&
      COPULAS.has(prev)
    )
      return t;
    if (
      words.slice(-MARK_DETERMINER_WINDOW).some((w) => MARK_DETERMINERS.has(w))
    )
      return t;
    if (mark === '?' && !QUESTION_STARTS.has(words[0])) return t;
    return body + mark;
  }
  return t;
}

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
 * On-device cleanup: spoken commands, fillers, stutters, spacing, "Scratch that" (ADR 0186), a final spoken "question
 * mark" / "exclamation point" (ADR 0189), capitalization, terminal punctuation (ADR 0101: which repeats collapse, and
 * no "." on a short dictation; ADR 0188: nor on a list of short lines).
 * Deterministic and instant; talk's only cleanup (no LLM pass, ADR 0071).
 */
export function ruleCleanup(
  raw: string,
  dict: readonly DictionaryEntry[] = [],
): string {
  let t = ` ${raw} `;
  t = t
    .replace(/\s*\bnew paragraph\b[,.]?\s*/gi, layoutCommand('\n\n'))
    .replace(/\s*\bnew line\b[,.]?\s*/gi, layoutCommand('\n'));
  t = t.replace(
    FILLERS,
    (all: string, sep: string, word: string, at: number, src: string) =>
      keepsFiller(word, all, src.slice(0, at)) ? all : sep,
  );
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
  t = scratchThat(t);
  t = finalMark(t);
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

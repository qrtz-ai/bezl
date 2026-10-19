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

// The filler word is group 3; whether it goes is keepsFiller's call (ADR 0182).
// Group 1 is a ", " right before the filler, put back as it was unless fillerTakesComma says it goes (ADR 0192).
const FILLERS =
  /(,(?= ))?(^|[\s,])(u+m+|u+h+|e+r+m+|e+r|a+h+|h+m+|m+h*m+)(?=[\s,.!?]|$)[,]?/gi;

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

/** ADR 0192: a filler the recogniser set off with a comma ("should, uh, ship") takes that comma with it only after a
 * word a comma can't follow in written English. Same lists and rule in cleanup.rs (`filler_takes_comma`). */
export const FILLER_COMMA_TIGHT = new Set(
  'the a an my your our their its every and but or nor because'.split(' '),
);
export const FILLER_COMMA_VERBS = new Set(
  'is are was were am can could should would will must might'.split(' '),
);
export const FILLER_COMMA_QUESTION_AUX = new Set(
  'can could will would should shall may might do does did is are was were have has'.split(
    ' ',
  ),
);
export const FILLER_COMMA_PRONOUNS = new Set(
  'i you we they he she it'.split(' '),
);
export const FILLER_COMMA_CLAUSE_OPENERS = new Set(
  (
    'if when whenever while because since as than unless until till though although whether once before after ' +
    'where wherever what whatever how however who whoever which whichever why that'
  ).split(' '),
);
export const FILLER_COMMA_REPLY_STARTS = new Set(
  'yes yeah yep no nope sure okay ok here there well oh'.split(' '),
);
export const FILLER_COMMA_NEXT_REFUSE = new Set(
  // pronouns
  (
    'i you we they he she it this that these those there here me us them him her nobody everyone everybody ' +
    'someone somebody anyone anybody nothing everything something anything none ' +
    // determiners
    'the a an my your our their his its some any every each no all both either neither ' +
    // conjunctions and clause openers
    'and but or nor so yet because if when while then though although unless since as which who whom whose ' +
    'what where why how whether than until once after before ' +
    // auxiliaries
    'do does did is are was were am be can could will would should shall may might must have has had ' +
    // discourse and sequence words
    'well okay ok yes yeah yep nope sure right like first second third next finally also plus again now ' +
    'anyway please thanks sorry oh too ' +
    // endearments and vocatives
    'honey babe baby dude man guys buddy sir bro mate folks sweetie dear love friend team boss'
  ).split(' '),
);
const FILLER_WORD = /^(u+m+|u+h+|e+r+m+|e+r|a+h+|h+m+|m+h*m+)$/i;

/** ADR 0192: does the comma before a removed filler go with it? `before` is the text up to that comma, `after` the
 * text after the filler (and its own comma, if any). */
function fillerTakesComma(before: string, after: string): boolean {
  const prev = before.split(/[ \n]/).at(-1) ?? '';
  if (!/^[A-Za-z]+$/.test(prev)) return false;
  // The next word: on the same line, a letter first, past any more lowercase fillers ("is, um, uh, basically").
  let rest = after.replace(/^ +/, '');
  let next: string;
  for (;;) {
    if (!/^[A-Za-z]/.test(rest)) return false;
    const tok = rest.split(/[ \n]/)[0] ?? '';
    const word = tok.replace(/[,.!?;:]+$/, '');
    const filler =
      /^[a-z]+$/.test(word) &&
      FILLER_WORD.test(word) &&
      /^,?$/.test(tok.slice(word.length));
    if (!filler) {
      next = word;
      break;
    }
    rest = rest.slice(tok.length).replace(/^ +/, '');
  }
  const low = prev.toLowerCase();
  // "a" only in lowercase: "Plan A, um," / "Vitamin A, um," are the letter.
  if (prev !== 'A' && FILLER_COMMA_TIGHT.has(low)) return true;
  const pronoun = FILLER_COMMA_PRONOUNS.has(low);
  if (!pronoun && !FILLER_COMMA_VERBS.has(low)) return false;
  if (
    !/^[a-z]*$/.test(next) ||
    FILLER_COMMA_NEXT_REFUSE.has(next) ||
    FILLER_WORD.test(next)
  )
    return false;
  // The sentence before the word: after the nearest line break or ". ", "! ", "? ", fillers left out.
  const head = before.slice(0, before.length - prev.length);
  let start = 0;
  for (let j = head.length - 1; j >= 1; j--) {
    if (
      head[j - 1] === '\n' ||
      (/[.!?]/.test(head[j - 1]) && head[j] === ' ')
    ) {
      start = j;
      break;
    }
  }
  const words = head
    .slice(start)
    .split(/\s+/)
    .map((w) => w.replace(/^[^A-Za-z0-9']+|[^A-Za-z0-9']+$/g, '').toLowerCase())
    .filter((w) => w !== '' && !FILLER_WORD.test(w));
  // "Can you, um, send": the pronoun is the second word of a question.
  if (pronoun)
    return words.length === 1 && FILLER_COMMA_QUESTION_AUX.has(words[0]);
  return (
    words.length > 0 &&
    !FILLER_COMMA_REPLY_STARTS.has(words[0]) &&
    !words.some((w) => FILLER_COMMA_CLAUSE_OPENERS.has(w))
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
// ADR 0193: number words, for spoken amounts. Same lists and rules in cleanup.rs (`spoken_amounts`).
const NUMBER_UNITS = 'zero one two three four five six seven eight nine'.split(
  ' ',
);
const NUMBER_TEENS =
  'ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen'.split(
    ' ',
  );
const NUMBER_TENS =
  'twenty thirty forty fifty sixty seventy eighty ninety'.split(' ');
/** ADR 0193: any of these (or a digit) near a spoken amount makes it part of a range, a decimal, a sum or a bigger
 * number: the words stay as spoken. Same list in cleanup.rs. */
export const AMOUNT_NEAR_REFUSE = new Set(
  'hundred thousand million billion trillion dozen point minus negative plus dot cent cents oh'.split(
    ' ',
  ),
);
const AMOUNT_WINDOW = 3;

const unitValue = (w: string): number | null => {
  const i = NUMBER_UNITS.indexOf(w);
  return i < 0 ? null : i;
};
const isNumberWord = (w: string) =>
  unitValue(w) !== null || NUMBER_TEENS.includes(w) || NUMBER_TENS.includes(w);

/** 1-99 from w[i..]: the value and the words used. */
function under100(w: readonly string[], i: number): [number, number] | null {
  const first = w[i];
  if (first === undefined) return null;
  const teen = NUMBER_TEENS.indexOf(first);
  if (teen >= 0) return [10 + teen, 1];
  const ten = NUMBER_TENS.indexOf(first);
  if (ten >= 0) {
    const tens = 20 + 10 * ten;
    const u = w[i + 1] === undefined ? null : unitValue(w[i + 1]);
    return u !== null && u > 0 ? [tens + u, 2] : [tens, 1];
  }
  const u = unitValue(first);
  return u !== null && u > 0 ? [u, 1] : null;
}

/** 1-999: "seven", "forty two", "three hundred", "three hundred and five", "three hundred twenty". */
function under1000(w: readonly string[], i: number): [number, number] | null {
  const u = w[i] === undefined ? null : unitValue(w[i]);
  if (u !== null && w[i + 1] === 'hundred') {
    if (u === 0) return null;
    if (w[i + 2] === 'and') {
      const r = under100(w, i + 3);
      return r ? [u * 100 + r[0], 3 + r[1]] : null;
    }
    const r = under100(w, i + 2);
    return r ? [u * 100 + r[0], 2 + r[1]] : [u * 100, 2];
  }
  return under100(w, i);
}

/** ADR 0193: the whole run as one number, 0-999,999, or null when any word is left over. */
function spokenNumber(w: readonly string[]): number | null {
  if (w.length === 1 && w[0] === 'zero') return 0;
  const a = under1000(w, 0);
  if (!a) return null;
  let i = a[1];
  if (i === w.length) return a[0];
  if (w[i] !== 'thousand') return null;
  i += 1;
  if (i === w.length) return a[0] * 1000;
  if (w[i] === 'and') i += 1;
  const b = under1000(w, i);
  return b && i + b[1] === w.length ? a[0] * 1000 + b[0] : null;
}

/** "15", "3.5": a plain decimal number written in digits. */
const isDecimal = (w: string) => /^[0-9]{1,3}(\.[0-9]+)?$/.test(w);

/** ADR 0193: a clock time before "a.m." / "p.m.": an hour 1-12, then nothing, "oh" and 1-9, or 10-59. */
function spokenClock(w: readonly string[]): string | null {
  const h = under100(w, 0);
  if (!h || h[1] !== 1 || h[0] < 1 || h[0] > 12) return null;
  const rest = w.slice(1);
  if (rest.length === 0) return String(h[0]);
  let m: number;
  if (rest[0] === 'oh') {
    const r = under100(rest, 1);
    if (!r || r[1] !== 1 || r[0] < 1 || r[0] > 9 || rest.length !== 2)
      return null;
    m = r[0];
  } else {
    const r = under100(rest, 0);
    if (!r || r[0] < 10 || r[0] > 59 || r[1] !== rest.length) return null;
    m = r[0];
  }
  return `${h[0]}:${String(m).padStart(2, '0')}`;
}

const grouped = (n: number) => String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
const bare = (w: string) =>
  w.replace(/^[^A-Za-z0-9]+|[^A-Za-z0-9]+$/g, '').toLowerCase();

/** ADR 0193: spoken amounts in their unambiguous forms ("twenty five dollars" → "$25", "fifteen percent" → "15%",
 * "three thirty p.m." → "3:30 p.m."). Same rule in cleanup.rs; runs last, after the terminal punctuation. */
function spokenAmounts(t: string): string {
  if (!(t.includes('dollars') || t.includes('percent') || t.includes('.m.')))
    return t;
  const toks: { s: number; e: number; line: number }[] = [];
  let line = 0;
  let start = -1;
  for (let i = 0; i < t.length; i++) {
    const c = t[i];
    if (/\s/.test(c)) {
      if (start >= 0) toks.push({ s: start, e: i, line });
      start = -1;
      if (c === '\n') line += 1;
    } else if (start < 0) start = i;
  }
  if (start >= 0) toks.push({ s: start, e: t.length, line });
  const word = (k: number) => t.slice(toks[k].s, toks[k].e);
  const nearNumber = (k: number) => {
    const b = bare(word(k));
    return /[0-9]/.test(b) || isNumberWord(b) || AMOUNT_NEAR_REFUSE.has(b);
  };
  const edits: [number, number, string][] = [];
  for (let k = 0; k < toks.length; k++) {
    const w = word(k);
    let unit: string;
    if (w.startsWith('dollars')) unit = '$';
    else if (w.startsWith('percent')) unit = '%';
    else if (w.startsWith('a.m.') || w.startsWith('p.m.')) unit = w.slice(0, 4);
    else continue;
    // "dollars" and "percent" are 7 letters, "a.m." / "p.m." 4.
    const tail = w.slice(unit.length === 1 ? 7 : 4);
    if (!/^[.,!?;:]*$/.test(tail)) continue;
    let j = k;
    while (j > 0 && toks[j - 1].line === toks[k].line) {
      const p = word(j - 1);
      const dec = unit === '%' && j === k && isDecimal(p);
      const plain = /^[a-z-]+$/.test(p) || dec;
      const numberish =
        p
          .toLowerCase()
          .split('-')
          .every(
            (q) =>
              isNumberWord(q) ||
              q === 'hundred' ||
              q === 'thousand' ||
              q === 'and' ||
              q === 'oh',
          ) || dec;
      if (!plain || !numberish) break;
      j -= 1;
    }
    // A leading "and" is the sentence's ("salt and fifteen percent"), not the number's.
    while (j < k && word(j).toLowerCase() === 'and') j += 1;
    if (j === k) continue;
    // A capitalized number word before the run: the sentence or a title starts there; the words stay.
    if (
      j > 0 &&
      toks[j - 1].line === toks[j].line &&
      /^[A-Z]/.test(word(j - 1)) &&
      isNumberWord(bare(word(j - 1)))
    )
      continue;
    let near = false;
    for (let q = Math.max(0, j - AMOUNT_WINDOW); q < j; q++)
      near ||= nearNumber(q);
    for (let q = k + 1; q < Math.min(toks.length, k + 1 + AMOUNT_WINDOW); q++)
      near ||= nearNumber(q);
    if (near) continue;
    const run: string[] = [];
    for (let q = j; q < k; q++) run.push(...word(q).toLowerCase().split('-'));
    let text: string | null = null;
    if (unit === '$') {
      const n = spokenNumber(run);
      text = n === null ? null : `$${grouped(n)}`;
    } else if (unit === '%') {
      if (run.length === 1 && isDecimal(run[0])) text = `${run[0]}%`;
      else {
        const n = spokenNumber(run);
        text = n === null ? null : `${grouped(n)}%`;
      }
    } else {
      const c = spokenClock(run);
      text = c === null ? null : `${c} ${unit}`;
    }
    if (text !== null) edits.push([toks[j].s, toks[k].e - tail.length, text]);
  }
  let out = t;
  for (const [a, b, text] of edits.reverse())
    out = out.slice(0, a) + text + out.slice(b);
  return out;
}

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
    (
      all: string,
      comma: string | undefined,
      sep: string,
      word: string,
      at: number,
      src: string,
    ) => {
      const c = comma ?? '';
      if (keepsFiller(word, all.slice(c.length), src.slice(0, at + c.length)))
        return all;
      return (c !== '' || sep === ',') &&
        fillerTakesComma(src.slice(0, at), src.slice(at + all.length))
        ? ' '
        : c + sep;
    },
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
  return spokenAmounts(terminalPunctuation(t));
}

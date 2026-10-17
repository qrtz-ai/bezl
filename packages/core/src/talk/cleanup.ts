import type { DictionaryEntry } from './contract';

const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
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
 * On-device cleanup: spoken commands, fillers, stutters, spacing, capitalization, terminal punctuation.
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
  // Immediate repeats ("I I think", "the the").
  t = t.replace(/\b(\w+)(?:[\s,]+\1\b)+/gi, '$1');
  t = t.replace(/[ \t]+/g, ' ').replace(/ *\n */g, '\n');
  t = t
    .replace(/\s+([,.!?;:])/g, '$1')
    .replace(/([,.!?;:])(?=[^\s\d"')\]])/g, '$1 ');
  t = t.replace(/,+/g, ',').replace(/^[\s,]+|[\s,]+$/g, '');
  t = t.replace(
    /(^|[.!?]\s+|\n)([a-z])/g,
    (_m, p: string, c: string) => p + c.toUpperCase(),
  );
  t = t.replace(/\bi\b(?=[\s,.!?']|$)/g, 'I');
  t = applyDictionary(t, dict);
  if (/[\p{L}\p{N}]$/u.test(t)) t += '.';
  return t;
}

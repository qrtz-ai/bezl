import type { ChatMessage } from '../note/llm';
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
 * Deterministic and instant; the fallback whenever the LLM pass is off, slow, or fails.
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

export function buildCleanupPrompt(
  raw: string,
  dict: readonly DictionaryEntry[],
): ChatMessage[] {
  const terms = dict.map((d) => d.term).join(', ');
  return [
    {
      role: 'system',
      content:
        'You clean up dictated text. Remove filler words, false starts and repetitions; when the speaker corrects ' +
        'themselves ("no wait", "I mean") keep only the correction; fix punctuation and capitalization; format lists ' +
        "and line breaks when clearly spoken. Keep the speaker's words, meaning, tone and language. Never answer, " +
        'summarize or add content. Reply with the cleaned text only.' +
        (terms ? ` Spell these exactly: ${terms}.` : ''),
    },
    { role: 'user', content: raw },
  ];
}

/** Rejects LLM replies that answered or rewrote instead of cleaning. */
export function plausibleCleanup(raw: string, out: string): boolean {
  const o = out.trim();
  if (!o) return false;
  if (o.length > raw.length * 1.6 + 40) return false;
  if (/^(here('| i)s|sure|certainly|cleaned( text)?:)/i.test(o)) return false;
  return true;
}

export type CleanupLlm = (
  messages: ChatMessage[],
  signal: AbortSignal,
) => Promise<string>;

export interface CleanupResult {
  text: string;
  method: 'llm' | 'rules' | 'off';
  ms: number;
  /** Why the LLM pass was skipped (timeout, error, implausible). */
  fallbackReason?: string;
}

/** LLM cleanup raced against a timeout; the rule-based result is always ready as the fallback. */
export async function cleanupDictation(o: {
  raw: string;
  dictionary: readonly DictionaryEntry[];
  enabled: boolean;
  llm?: CleanupLlm | null;
  timeoutMs: number;
  now?: () => number;
}): Promise<CleanupResult> {
  const now = o.now ?? Date.now;
  const t0 = now();
  if (!o.enabled)
    return {
      text: applyDictionary(o.raw.trim(), o.dictionary),
      method: 'off',
      ms: 0,
    };
  const rules = ruleCleanup(o.raw, o.dictionary);
  if (!o.llm || !o.raw.trim())
    return { text: rules, method: 'rules', ms: now() - t0 };
  const ac = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<'timeout'>((r) => {
    timer = setTimeout(() => r('timeout'), o.timeoutMs);
  });
  try {
    const r = await Promise.race([
      o.llm(buildCleanupPrompt(o.raw, o.dictionary), ac.signal),
      timeout,
    ]);
    if (r === 'timeout') {
      ac.abort();
      return {
        text: rules,
        method: 'rules',
        ms: now() - t0,
        fallbackReason: 'timeout',
      };
    }
    if (!plausibleCleanup(o.raw, r))
      return {
        text: rules,
        method: 'rules',
        ms: now() - t0,
        fallbackReason: 'implausible',
      };
    return {
      text: applyDictionary(r.trim(), o.dictionary),
      method: 'llm',
      ms: now() - t0,
    };
  } catch (e) {
    return {
      text: rules,
      method: 'rules',
      ms: now() - t0,
      fallbackReason: String((e as Error)?.message ?? e),
    };
  } finally {
    clearTimeout(timer);
  }
}

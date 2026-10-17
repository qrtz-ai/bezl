export interface Command {
  id: string;
  title: string;
  hint?: string;
  /** Feather icon name from @qrtz/ui ICONS. */
  icon?: string;
  keywords?: string;
  run: () => void | Promise<unknown>;
}

/** Subsequence fuzzy score: higher is better, null if no match. Prefers word-start and contiguous hits. */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.toLowerCase().trim();
  if (!q) return 0;
  const s = text.toLowerCase();
  let score = 0;
  let last = -1;
  for (const ch of q) {
    if (ch === ' ') continue;
    const i = s.indexOf(ch, last + 1);
    if (i < 0) return null;
    score += i === last + 1 ? 3 : 1;
    if (i === 0 || s[i - 1] === ' ') score += 2;
    last = i;
  }
  return score - s.length * 0.01;
}

export function filterCommands(cmds: Command[], q: string): Command[] {
  return cmds
    .map((c) => ({ c, s: fuzzyScore(q, `${c.title} ${c.keywords ?? ''}`) }))
    .filter((x): x is { c: Command; s: number } => x.s !== null)
    .sort((a, b) => b.s - a.s)
    .map((x) => x.c);
}

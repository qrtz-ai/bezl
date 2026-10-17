import { z } from 'zod';
import {
  fenceBlock,
  fenceNonce,
  promptSafe,
  type LlmProvider,
  type ChatMessage,
} from './llm';
import { bigrams, CHAT_MAX_BIGRAMS, UNSEGMENTED } from './unsegmented';

/** One retrieved transcript line given to the model (and cited back to the UI). */
export const ChatSource = z.object({
  meetingId: z.string(),
  title: z.string(),
  startedAt: z.number(),
  startMs: z.number(),
  speaker: z.string().optional(),
  text: z.string(),
});
export type ChatSource = z.infer<typeof ChatSource>;

export const ChatAnswer = z.object({
  answer: z.string(),
  /** Sources the answer is grounded in (numbered [1], [2]… in `answer` when the LLM cites). */
  sources: z.array(ChatSource),
  provider: z.string(),
  /** Set when the LLM failed and this is the extractive fallback. */
  llmError: z.string().max(500).optional(),
});
export type ChatAnswer = z.infer<typeof ChatAnswer>;

export const ChatTurn = z.object({
  role: z.enum(['user', 'assistant']),
  content: z.string().max(8000),
});
export type ChatTurn = z.infer<typeof ChatTurn>;

const STOP = new Set(
  'a an the and or but of to in on at for with is are was were be been it this that what who when where why how did do does i you we they he she me my our your about from as by can could should would will any all'.split(
    ' ',
  ),
);
/**
 * Question terms, split the same way as the source words in `rankSources` (on anything but letters, digits and
 * combining marks; a mark inside a Thai or Indic word no longer cuts it, N49).
 * They used to drop apostrophes instead, so "O'Reilly" became `oreilly`, which no source word (`o`, `reilly`) starts
 * with, and the line FTS had just retrieved scored 0 and was dropped (N47, ADR 0038).
 */
export const queryTerms = (q: string) => {
  const words = [
    ...new Set(
      q
        .toLowerCase()
        .normalize('NFKC')
        .split(/[^\p{L}\p{N}\p{M}]+/u)
        .filter((w) => w.length > 1 && !STOP.has(w)),
    ),
  ].slice(0, 12);
  // N49 (ADR 0047): a run of Han, kana or Thai is one "word" here (a whole Chinese question has no spaces), and no
  // line word starts with it. Its bigrams are matched inside the line instead, the same ones retrieval used.
  const plain = words.filter((w) => !UNSEGMENTED.test(w));
  const grams = [
    ...new Set(words.filter((w) => UNSEGMENTED.test(w)).flatMap(bigrams)),
  ].slice(0, CHAT_MAX_BIGRAMS);
  return [...plain, ...grams];
};

/** Cheap lexical relevance: matched distinct terms (prefix match), weighted by rarity across the candidates. */
export function rankSources(
  question: string,
  sources: ChatSource[],
  k = 24,
): ChatSource[] {
  const terms = queryTerms(question);
  if (!terms.length) return sources.slice(0, k);
  const words = sources.map((s) =>
    `${s.speaker ?? ''} ${s.text}`
      .toLowerCase()
      .normalize('NFKC')
      .split(/[^\p{L}\p{N}\p{M}]+/u),
  );
  const lines = words.map((ws) => ws.join(' '));
  // A bigram (N49) is matched anywhere in the line, any other term as a word prefix.
  const has = (i: number, t: string) =>
    UNSEGMENTED.test(t)
      ? lines[i]!.includes(t)
      : words[i]!.some((w) => w.startsWith(t));
  const df = terms.map((t) => sources.filter((_, i) => has(i, t)).length);
  const scored = sources.map((s, i) => {
    let score = 0;
    terms.forEach((t, j) => {
      if (has(i, t)) score += Math.log(1 + sources.length / (1 + df[j]!));
    });
    return { s, score, i };
  });
  return scored
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score || a.i - b.i)
    .slice(0, k)
    .map((x) => x.s);
}

const ts = (ms: number) => {
  const t = Math.floor(ms / 1000);
  return `${Math.floor(t / 60)}:${String(t % 60).padStart(2, '0')}`;
};

export function buildChatPrompt(
  question: string,
  sources: ChatSource[],
  history: ChatTurn[] = [],
  nonce: string = fenceNonce(),
): ChatMessage[] {
  // N69: excerpts are one line each and fenced, so a transcript line can't pose as an instruction or excerpt.
  const ctx = sources
    .map(
      (s, i) =>
        `[${i + 1}] (${promptSafe(s.title)} @ ${ts(s.startMs)}) ${promptSafe(s.speaker ?? 'Speaker')}: ${promptSafe(s.text)}`,
    )
    .join('\n');
  const fence = fenceBlock('excerpts', ctx || '(none)', nonce, 'excerpt list');
  return [
    {
      role: 'system',
      content:
        "You answer questions about the user's meetings using ONLY the numbered transcript excerpts provided. Cite excerpts inline like [2]. If the excerpts do not contain the answer, say you could not find it in the meetings. Be concise." +
        fence.rule,
    },
    ...history.slice(-6).map((h) => ({ role: h.role, content: h.content })),
    {
      role: 'user',
      content: `Excerpts:\n${fence.block}\n\nQuestion: ${question}`,
    },
  ];
}

/** Without an LLM: quote the most relevant lines verbatim. */
export function extractiveAnswer(
  question: string,
  sources: ChatSource[],
): ChatAnswer {
  return quoteAnswer(rankSources(question, sources, 3));
}

function quoteAnswer(top: ChatSource[]): ChatAnswer {
  if (!top.length)
    return {
      answer: "I couldn't find anything about that in your meetings.",
      sources: [],
      provider: 'extractive',
    };
  return {
    answer: top
      .map(
        (s, i) =>
          `[${i + 1}] ${s.speaker ?? 'Someone'} (${s.title}, ${ts(s.startMs)}): "${s.text}"`,
      )
      .join('\n'),
    sources: top,
    provider: 'extractive',
  };
}

/** Grounded Q&A over retrieved transcript lines. LLM when configured, extractive quotes otherwise. Never throws. */
export async function answerQuestion(
  question: string,
  candidates: ChatSource[],
  provider: LlmProvider | null,
  history: ChatTurn[] = [],
  /** Candidates are already ranked (hybrid retrieval); use their order instead of lexical ranking. */
  preRanked = false,
): Promise<ChatAnswer> {
  const sources = preRanked
    ? candidates.slice(0, 24)
    : rankSources(question, candidates, 24);
  const fallback = () =>
    preRanked
      ? quoteAnswer(sources.slice(0, 3))
      : extractiveAnswer(question, candidates);
  if (!provider) return fallback();
  try {
    const answer = await provider.complete(
      buildChatPrompt(question, sources, history),
    );
    return { answer: answer.trim(), sources, provider: provider.id };
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    return {
      ...fallback(),
      llmError: msg.slice(0, 500),
    };
  }
}

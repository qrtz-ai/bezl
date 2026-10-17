import { z } from 'zod';
import type { LlmProvider, ChatMessage } from './llm';

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
export const queryTerms = (q: string) =>
  [
    ...new Set(
      q
        .toLowerCase()
        .normalize('NFKC')
        .split(/[^\p{L}\p{N}']+/u)
        .map((w) => w.replace(/'/g, ''))
        .filter((w) => w.length > 1 && !STOP.has(w)),
    ),
  ].slice(0, 12);

/** Cheap lexical relevance: matched distinct terms (prefix match), weighted by rarity across the candidates. */
export function rankSources(
  question: string,
  sources: ChatSource[],
  k = 24,
): ChatSource[] {
  const terms = queryTerms(question);
  if (!terms.length) return sources.slice(0, k);
  const words = sources.map((s) =>
    `${s.speaker ?? ''} ${s.text}`.toLowerCase().split(/[^\p{L}\p{N}]+/u),
  );
  const df = terms.map(
    (t) => words.filter((ws) => ws.some((w) => w.startsWith(t))).length,
  );
  const scored = sources.map((s, i) => {
    let score = 0;
    terms.forEach((t, j) => {
      if (words[i]!.some((w) => w.startsWith(t)))
        score += Math.log(1 + sources.length / (1 + df[j]!));
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
): ChatMessage[] {
  const ctx = sources
    .map(
      (s, i) =>
        `[${i + 1}] (${s.title} @ ${ts(s.startMs)}) ${s.speaker ?? 'Speaker'}: ${s.text}`,
    )
    .join('\n');
  return [
    {
      role: 'system',
      content:
        "You answer questions about the user's meetings using ONLY the numbered transcript excerpts provided. Cite excerpts inline like [2]. If the excerpts do not contain the answer, say you could not find it in the meetings. Be concise.",
    },
    ...history.slice(-6).map((h) => ({ role: h.role, content: h.content })),
    {
      role: 'user',
      content: `Excerpts:\n${ctx || '(none)'}\n\nQuestion: ${question}`,
    },
  ];
}

/** Without an LLM: quote the most relevant lines verbatim. */
export function extractiveAnswer(
  question: string,
  sources: ChatSource[],
): ChatAnswer {
  const top = rankSources(question, sources, 3);
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
): Promise<ChatAnswer> {
  const sources = rankSources(question, candidates, 24);
  if (!provider) return extractiveAnswer(question, candidates);
  try {
    const answer = await provider.complete(
      buildChatPrompt(question, sources, history),
    );
    return { answer: answer.trim(), sources, provider: provider.id };
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    return {
      ...extractiveAnswer(question, candidates),
      llmError: msg.slice(0, 500),
    };
  }
}

import { z } from 'zod';
import { AppError } from '../errors';
import type { Segment } from './types';
import { MeetingSummary } from './summary';
import { extractiveSummary } from './extractive';
import { formatNotesForPrompt, type MeetingNote } from './notes';
import { templateById } from './templates';

export interface SummaryOptions {
  notes?: MeetingNote[];
  template?: string;
  /** Rethrow provider failures instead of falling back (the lifecycle pipeline owns retry + fallback). */
  strict?: boolean;
}

export type ChatMessage = {
  role: 'system' | 'user' | 'assistant';
  content: string;
};
export type FetchLike = (
  url: string,
  init: {
    method: string;
    headers: Record<string, string>;
    body: string;
    signal?: AbortSignal;
  },
) => Promise<{
  ok: boolean;
  status: number;
  json(): Promise<unknown>;
  text(): Promise<string>;
}>;

export interface LlmProvider {
  readonly id: string;
  complete(
    messages: ChatMessage[],
    opts?: { json?: boolean; signal?: AbortSignal },
  ): Promise<string>;
}

export type ProviderId =
  | 'openai'
  | 'anthropic'
  | 'google'
  | 'openrouter'
  | 'ollama'
  | 'nvidia';

/**
 * Default model for every LLM use in qrtz (note summary + final note, chat/ask, talk cleanup) via OpenRouter.
 * Slug verified against https://openrouter.ai/api/v1/models (Oct 2026).
 */
export const OPENROUTER_DEFAULT_MODEL = 'anthropic/claude-haiku-5.5';

export const PROVIDER_DEFAULTS: Record<
  ProviderId,
  { baseUrl: string; model: string; secret?: string }
> = {
  openai: {
    baseUrl: 'https://api.openai.com/v1',
    model: 'gpt-4.1-mini',
    secret: 'llm.openai',
  },
  openrouter: {
    baseUrl: 'https://openrouter.ai/api/v1',
    model: OPENROUTER_DEFAULT_MODEL,
    secret: 'llm.openrouter',
  },
  nvidia: {
    baseUrl: 'https://integrate.api.nvidia.com/v1',
    model: 'nvidia/llama-3.3-nemotron-super-49b-v1',
    secret: 'asr.nvidia',
  },
  ollama: { baseUrl: 'http://127.0.0.1:11434/v1', model: 'llama3.2' },
  anthropic: {
    baseUrl: 'https://api.anthropic.com/v1',
    model: 'claude-sonnet-4-5',
    secret: 'llm.anthropic',
  },
  google: {
    baseUrl: 'https://generativelanguage.googleapis.com/v1beta',
    model: 'gemini-2.5-flash',
    secret: 'llm.google',
  },
};

async function post(
  fetchFn: FetchLike,
  url: string,
  headers: Record<string, string>,
  body: unknown,
  signal?: AbortSignal,
) {
  let res: Awaited<ReturnType<FetchLike>>;
  try {
    res = await fetchFn(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json', ...headers },
      body: JSON.stringify(body),
      ...(signal ? { signal } : {}),
    });
  } catch (e) {
    // Transport failures (DNS, offline, reset) must be retryable: F11 / summary policy.
    // A raw TypeError would become E_INTERNAL via AppError.from and fail the step once.
    const msg = e instanceof Error ? e.message : String(e);
    const aborted =
      (e instanceof Error && e.name === 'AbortError') ||
      (typeof DOMException !== 'undefined' &&
        e instanceof DOMException &&
        e.name === 'AbortError') ||
      signal?.aborted;
    throw new AppError(
      aborted ? 'E_TIMEOUT' : 'E_UNAVAILABLE',
      `LLM network error: ${msg.slice(0, 200)}`,
      { retryable: true, cause: e },
    );
  }
  if (!res.ok) {
    const t = await res.text().catch(() => '');
    // Providers return {"error":{"message":...}}; show that instead of raw JSON.
    let detail = t.slice(0, 200);
    try {
      const j = JSON.parse(t) as { error?: { message?: unknown } | string };
      const m = typeof j.error === 'string' ? j.error : j.error?.message;
      if (typeof m === 'string' && m) detail = m.slice(0, 200);
    } catch {
      /* not JSON */
    }
    const hint =
      res.status === 401 || res.status === 403
        ? ' (check the API key in Settings)'
        : res.status === 402
          ? ' (no OpenRouter credits: add credits at openrouter.ai/settings/credits, then retry)'
          : res.status === 429
            ? ' (rate limited; try again shortly)'
            : '';
    throw new AppError(
      'E_PROVIDER',
      `LLM HTTP ${res.status}: ${detail}${hint}`,
      { retryable: res.status >= 500 || res.status === 429 },
    );
  }
  return res.json();
}

/** OpenAI Chat Completions–compatible (OpenAI, OpenRouter, NVIDIA NIM, Groq, Ollama, LM Studio...). */
export function openAiCompatible(o: {
  id: string;
  fetch: FetchLike;
  baseUrl: string;
  apiKey?: string;
  model: string;
  extraHeaders?: Record<string, string>;
}): LlmProvider {
  return {
    id: o.id,
    async complete(messages, opts) {
      const j = (await post(
        o.fetch,
        `${o.baseUrl}/chat/completions`,
        {
          ...o.extraHeaders,
          ...(o.apiKey ? { authorization: `Bearer ${o.apiKey}` } : {}),
        },
        {
          model: o.model,
          messages,
          temperature: 0.2,
          ...(opts?.json ? { response_format: { type: 'json_object' } } : {}),
        },
        opts?.signal,
      )) as { choices?: { message?: { content?: string } }[] };
      return j.choices?.[0]?.message?.content ?? '';
    },
  };
}

export function anthropic(o: {
  fetch: FetchLike;
  apiKey: string;
  model: string;
  baseUrl?: string;
}): LlmProvider {
  return {
    id: 'anthropic',
    async complete(messages, opts) {
      const system = messages
        .filter((m) => m.role === 'system')
        .map((m) => m.content)
        .join('\n');
      const j = (await post(
        o.fetch,
        `${o.baseUrl ?? PROVIDER_DEFAULTS.anthropic.baseUrl}/messages`,
        { 'x-api-key': o.apiKey, 'anthropic-version': '2023-06-01' },
        {
          model: o.model,
          max_tokens: 2048,
          system,
          messages: messages.filter((m) => m.role !== 'system'),
        },
        opts?.signal,
      )) as { content?: { type: string; text?: string }[] };
      return j.content?.find((c) => c.type === 'text')?.text ?? '';
    },
  };
}

export function google(o: {
  fetch: FetchLike;
  apiKey: string;
  model: string;
  baseUrl?: string;
}): LlmProvider {
  return {
    id: 'google',
    async complete(messages, opts) {
      const system = messages
        .filter((m) => m.role === 'system')
        .map((m) => m.content)
        .join('\n');
      const j = (await post(
        o.fetch,
        `${o.baseUrl ?? PROVIDER_DEFAULTS.google.baseUrl}/models/${o.model}:generateContent`,
        { 'x-goog-api-key': o.apiKey },
        {
          systemInstruction: { parts: [{ text: system }] },
          contents: messages
            .filter((m) => m.role !== 'system')
            .map((m) => ({
              role: m.role === 'assistant' ? 'model' : 'user',
              parts: [{ text: m.content }],
            })),
          generationConfig: opts?.json
            ? { responseMimeType: 'application/json' }
            : {},
        },
        opts?.signal,
      )) as { candidates?: { content?: { parts?: { text?: string }[] } }[] };
      return (
        j.candidates?.[0]?.content?.parts?.map((p) => p.text ?? '').join('') ??
        ''
      );
    },
  };
}

/** True when an error means "this account can't pay for that model" (not a bad key or an outage). */
export const needsCredits = (e: unknown): boolean => {
  const m = e instanceof Error ? e.message : String(e);
  return (
    /LLM HTTP 402\b/.test(m) ||
    /\b(credits?|insufficient|requires? (?:more )?payment)\b/i.test(m)
  );
};

/** Builds a provider from config + key. Returns null when no usable provider is configured. */
export function createProvider(
  id: ProviderId | 'none',
  o: { fetch: FetchLike; apiKey?: string; model?: string; baseUrl?: string },
): LlmProvider | null {
  if (id === 'none') return null;
  const d = PROVIDER_DEFAULTS[id];
  const model = o.model || d.model;
  const baseUrl = o.baseUrl || d.baseUrl;
  if (d.secret && !o.apiKey) return null;
  if (id === 'anthropic')
    return anthropic({ fetch: o.fetch, apiKey: o.apiKey!, model, baseUrl });
  if (id === 'google')
    return google({ fetch: o.fetch, apiKey: o.apiKey!, model, baseUrl });
  const make = (m: string) =>
    openAiCompatible({
      id,
      fetch: o.fetch,
      baseUrl,
      model: m,
      ...(o.apiKey ? { apiKey: o.apiKey } : {}),
      ...(id === 'openrouter'
        ? {
            extraHeaders: {
              'HTTP-Referer': 'https://qrtz.ai',
              'X-Title': 'qrtz',
            },
          }
        : {}),
    });
  // No silent model swap: a 402 (no credits) surfaces as an error with retry; summaries keep the on-device fallback.
  return make(model);
}

export function formatTranscriptForPrompt(
  segments: Segment[],
  maxChars = 60_000,
): string {
  const lines = segments.map((s) => {
    const t = Math.floor(s.startMs / 1000);
    const ts = `${Math.floor(t / 60)}:${String(t % 60).padStart(2, '0')}`;
    return `[${ts}] ${s.speakerName ?? s.speaker ?? 'Speaker'}: ${s.text}`;
  });
  let out = lines.join('\n');
  if (out.length > maxChars)
    out = out.slice(0, maxChars / 2) + '\n…\n' + out.slice(-maxChars / 2);
  return out;
}

export function buildSummaryPrompt(
  title: string,
  segments: Segment[],
  opts: SummaryOptions = {},
): ChatMessage[] {
  const tpl = templateById(opts.template);
  const notes = (opts.notes ?? []).filter((n) => n.text.trim());
  return [
    {
      role: 'system',
      content:
        'You summarize meeting transcripts. Reply with ONLY a JSON object: {"summary": string (3-6 sentences), "actionItems": [{"text": string, "owner"?: string}], "qa": [{"question": string, "answer": string, "askedBy"?: string}]}. Use speaker names from the transcript. Do not invent facts. ' +
        tpl.guidance +
        (notes.length
          ? " The user took their own notes during the meeting (timestamps match the transcript). Treat them as the user's priorities: build the summary around them, expand each with details from the transcript, keep any to-dos from the notes as action items, and never contradict the transcript."
          : ''),
    },
    {
      role: 'user',
      content:
        `Meeting: ${title}\n\n` +
        (notes.length ? `My notes:\n${formatNotesForPrompt(notes)}\n\n` : '') +
        `Transcript:\n${formatTranscriptForPrompt(segments)}`,
    },
  ];
}

const LlmSummaryJson = MeetingSummary.omit({ provider: true, llmError: true });

/** Parses a model reply that should be JSON (tolerates code fences / leading prose). */
export function parseSummaryReply(
  text: string,
): z.infer<typeof LlmSummaryJson> {
  const start = text.indexOf('{');
  const end = text.lastIndexOf('}');
  if (start < 0 || end <= start)
    throw new AppError('E_PROVIDER', 'LLM reply had no JSON object');
  return LlmSummaryJson.parse(JSON.parse(text.slice(start, end + 1)));
}

/** LLM summary with automatic fallback to the local extractive summarizer. Never throws. */
export async function summarizeMeeting(
  title: string,
  segments: Segment[],
  provider: LlmProvider | null,
  onError?: (e: unknown) => void,
  opts: SummaryOptions = {},
): Promise<MeetingSummary> {
  const template = templateById(opts.template).id;
  const hasNotes = !!opts.notes?.some((n) => n.text.trim());
  if (provider && (segments.length || hasNotes)) {
    try {
      const reply = await provider.complete(
        buildSummaryPrompt(title, segments, opts),
        { json: true },
      );
      return { ...parseSummaryReply(reply), provider: provider.id, template };
    } catch (e) {
      onError?.(e);
      if (opts.strict) throw e;
      const msg = e instanceof Error ? e.message : String(e);
      return {
        ...extractiveSummary(segments, opts),
        llmError: msg.slice(0, 500),
      };
    }
  }
  return extractiveSummary(segments, opts);
}

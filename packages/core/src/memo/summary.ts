import { z } from 'zod';

export const MeetingSummary = z.object({
  summary: z.string(),
  actionItems: z.array(
    z.object({ text: z.string(), owner: z.string().optional() }),
  ),
  qa: z.array(
    z.object({
      question: z.string(),
      answer: z.string(),
      askedBy: z.string().optional(),
    }),
  ),
  provider: z.string(),
  /** Template used to produce this summary. */
  template: z.string().optional(),
  /** Set when the configured LLM failed and this is the extractive fallback; the UI offers a retry. */
  llmError: z.string().max(500).optional(),
});
export type MeetingSummary = z.infer<typeof MeetingSummary>;

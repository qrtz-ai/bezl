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
});
export type MeetingSummary = z.infer<typeof MeetingSummary>;

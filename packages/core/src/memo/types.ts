import { z } from 'zod';

export const MeetingPlatform = z.enum([
  'zoom',
  'google-meet',
  'teams',
  'slack-huddle',
  'webex',
  'other',
]);
export type MeetingPlatform = z.infer<typeof MeetingPlatform>;

/** Times are ms offsets from recording start unless named `*At` (epoch ms). */
export const Segment = z.object({
  id: z.string(),
  startMs: z.number().nonnegative(),
  endMs: z.number().nonnegative(),
  text: z.string(),
  /** Diarization slot (e.g. "S0") before fusion; resolved name after. */
  speaker: z.string().optional(),
  speakerName: z.string().optional(),
  source: z.enum(['live', 'post']),
  confidence: z.number().min(0).max(1).optional(),
});
export type Segment = z.infer<typeof Segment>;

export const ActiveSpeakerEvent = z.object({
  atMs: z.number(),
  name: z.string(),
});
export type ActiveSpeakerEvent = z.infer<typeof ActiveSpeakerEvent>;

export const SpeakerTurn = z.object({
  slot: z.string(),
  startMs: z.number(),
  endMs: z.number(),
});
export type SpeakerTurn = z.infer<typeof SpeakerTurn>;

export const Meeting = z.object({
  id: z.string(),
  title: z.string(),
  platform: MeetingPlatform,
  startedAt: z.number(),
  endedAt: z.number().optional(),
  participants: z.array(z.string()).default([]),
  status: z.enum(['recording', 'processing', 'done', 'failed']),
  audioPath: z.string().optional(),
});
export type Meeting = z.infer<typeof Meeting>;

/** ASR stream events from the helper. `partial` replaces the current hypothesis for its utterance. */
export const AsrEvent = z.discriminatedUnion('kind', [
  z.object({
    kind: z.literal('partial'),
    utteranceId: z.string(),
    startMs: z.number(),
    text: z.string(),
  }),
  z.object({
    kind: z.literal('final'),
    utteranceId: z.string(),
    startMs: z.number(),
    endMs: z.number(),
    text: z.string(),
    confidence: z.number().optional(),
  }),
]);
export type AsrEvent = z.infer<typeof AsrEvent>;

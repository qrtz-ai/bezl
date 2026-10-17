import { z } from 'zod';
import { OsPermission } from '../capabilities';
import { Meeting, MeetingPlatform, Segment } from './types';
import { MeetingSummary } from './summary';
import { MeetingNote } from './notes';
import {
  Degradation,
  LifecycleError,
  PipelineView,
  RecordingPhase,
  StopReason,
} from './lifecycle';

export const MeetingWithDuration = Meeting.extend({
  durationMs: z.number().optional(),
});

export const SessionView = z.object({
  phase: RecordingPhase,
  detected: z
    .object({
      key: z.string(),
      title: z.string(),
      platform: MeetingPlatform,
      app: z.string(),
    })
    .optional(),
  meetingId: z.string().optional(),
  startedAt: z.number().optional(),
  elapsedMs: z.number().default(0),
  participants: z.array(z.string()).default([]),
  activeSpeaker: z.string().optional(),
  level: z.number().default(0),
  /** Recent finalized lines for the HUD (last ~6). */
  lines: z
    .array(
      z.object({
        id: z.string(),
        speaker: z.string().optional(),
        text: z.string(),
        startMs: z.number(),
      }),
    )
    .default([]),
  /** Current in-flight partial hypothesis. */
  partial: z.string().default(''),
  progress: z.string().optional(),
  error: z.string().optional(),
  /**
   * Native capture helper health: ok (real helper running), simulated (mock: Linux/dev, no real capture),
   * missing (macOS build without the helper binary), down (crashed / restart failed).
   */
  helper: z.enum(['ok', 'simulated', 'missing', 'down']).default('ok'),
  helperDetail: z.string().optional(),
  /** Capture paused by the user (privacy hotkey): nothing is transcribed, audio is silence. */
  paused: z.boolean().default(false),
  stopReason: StopReason.optional(),
  /** Capture is running but impaired (mic-only, no ASR, no AX, gap after a helper restart). */
  degraded: z.array(Degradation).default([]),
  /** Structured error (phase error, or a failed start that returned to idle/detected). */
  errorInfo: LifecycleError.optional(),
  /** Helper restart in progress mid-recording. */
  reconnect: z
    .object({ attempt: z.number(), since: z.number(), deadlineAt: z.number() })
    .optional(),
  /** Post-processing progress of the session's meeting, per step. */
  pipeline: PipelineView.optional(),
});
export type SessionView = z.infer<typeof SessionView>;

export const PermissionState = z.enum([
  'granted',
  'denied',
  'not-determined',
  'unsupported',
  'unknown',
]);
export const PermissionsStatus = z.record(OsPermission, PermissionState);
export type PermissionsStatus = z.infer<typeof PermissionsStatus>;

export const SearchHitSchema = z.object({
  meetingId: z.string(),
  title: z.string(),
  startedAt: z.number(),
  snippet: z.string(),
  startMs: z.number().optional(),
});

export const MeetingListItem = z.object({
  meeting: MeetingWithDuration,
  hit: SearchHitSchema.optional(),
});
export const MeetingDetail = z.object({
  meeting: MeetingWithDuration,
  segments: z.array(Segment),
  summary: MeetingSummary.optional(),
  notes: z.array(MeetingNote).default([]),
});
export type MeetingDetail = z.infer<typeof MeetingDetail>;

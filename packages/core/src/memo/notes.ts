import { z } from 'zod';

/** A user's live note; `atMs` is the offset into the recording when it was written (0 for after-the-fact notes). */
export const MeetingNote = z.object({
  id: z.string(),
  atMs: z.number().nonnegative(),
  text: z.string(),
  createdAt: z.number(),
});
export type MeetingNote = z.infer<typeof MeetingNote>;

export const clockLabel = (ms: number) => {
  const t = Math.floor(ms / 1000);
  const h = Math.floor(t / 3600);
  const m = Math.floor(t / 60) % 60;
  const s = String(t % 60).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
};

export function formatNotesForPrompt(notes: MeetingNote[]): string {
  return notes.map((n) => `[${clockLabel(n.atMs)}] ${n.text}`).join('\n');
}

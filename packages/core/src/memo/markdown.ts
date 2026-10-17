import type { Meeting, Segment } from './types';
import type { MeetingSummary } from './summary';

export const fmtClock = (ms: number) => {
  const t = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const s = String(t % 60).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
};

const PLATFORM: Record<string, string> = {
  zoom: 'Zoom',
  'google-meet': 'Google Meet',
  teams: 'Microsoft Teams',
  'slack-huddle': 'Slack huddle',
  webex: 'Webex',
  other: 'Meeting',
};

export function slugify(s: string): string {
  return (
    s
      .toLowerCase()
      .normalize('NFKD')
      .replace(/[^\p{L}\p{N}]+/gu, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 60) || 'meeting'
  );
}

export function exportFilename(
  m: Pick<Meeting, 'title' | 'startedAt'>,
): string {
  return `${new Date(m.startedAt).toISOString().slice(0, 10)}-${slugify(m.title)}.md`;
}

const esc = (s: string) => s.replace(/([\\`*_[\]#<>|])/g, '\\$1');

/** Deterministic Markdown export (golden-tested). Times shown in UTC ISO for portability. */
export function meetingToMarkdown(
  m: Meeting & { durationMs?: number },
  segments: Segment[],
  summary?: MeetingSummary,
): string {
  const out: string[] = [];
  out.push(`# ${esc(m.title)}`, '');
  out.push(
    `- **When:** ${new Date(m.startedAt).toISOString().replace('T', ' ').slice(0, 16)} UTC`,
  );
  out.push(`- **Platform:** ${PLATFORM[m.platform] ?? m.platform}`);
  if (m.durationMs != null)
    out.push(`- **Duration:** ${fmtClock(m.durationMs)}`);
  if (m.participants.length)
    out.push(`- **Participants:** ${m.participants.map(esc).join(', ')}`);
  out.push('');
  if (summary) {
    out.push('## Summary', '', esc(summary.summary), '');
    out.push('## Action items', '');
    if (summary.actionItems.length)
      for (const a of summary.actionItems)
        out.push(
          `- [ ] ${esc(a.text)}${a.owner ? ` — _${esc(a.owner)}_` : ''}`,
        );
    else out.push('_None._');
    out.push('');
    if (summary.qa.length) {
      out.push('## Q&A', '');
      for (const q of summary.qa)
        out.push(
          `**Q${q.askedBy ? ` (${esc(q.askedBy)})` : ''}:** ${esc(q.question)}  `,
          `**A:** ${esc(q.answer)}`,
          '',
        );
    }
  }
  out.push('## Transcript', '');
  for (const s of segments)
    out.push(
      `**[${fmtClock(s.startMs)}] ${esc(s.speakerName ?? s.speaker ?? 'Speaker')}:** ${esc(s.text)}`,
      '',
    );
  out.push(
    `---`,
    `_Exported from qrtz Memo · summary by ${summary?.provider ?? 'n/a'}_`,
    '',
  );
  return out.join('\n');
}

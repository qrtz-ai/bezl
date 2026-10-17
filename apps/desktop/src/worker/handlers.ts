import type { DatabaseSync } from 'node:sqlite';
import {
  AppError,
  MemoRepo,
  createProvider,
  exportFilename,
  nameSegmentsFromAx,
  meetingToMarkdown,
  summarizeMeeting,
  type ActiveSpeakerEvent,
  type FetchLike,
  type MeetingRow,
  type SearchHit,
  type MeetingSummary,
  type ProviderId,
  type Segment,
} from '@qrtz/core';
import { dbHealth } from './db';

export interface FinalizeParams {
  meetingId: string;
  endedAt: number;
  durationMs: number;
  audioPath?: string;
  active: ActiveSpeakerEvent[];
  llm: { provider: ProviderId | 'none'; model?: string; apiKey?: string };
}

/** All worker RPC methods. Pure over a DB + fetch so they're integration-testable in Node. */
export function createWorkerHandlers(
  db: DatabaseSync,
  deps: { fetch: FetchLike; log?: (msg: string, f?: object) => void },
) {
  const repo = new MemoRepo(db);
  const need = (id: string) => {
    const m = repo.getMeeting(id);
    if (!m) throw new AppError('E_NOT_FOUND', `meeting ${id} not found`);
    return m;
  };
  return {
    'db.health': () => dbHealth(db),
    'memo.create': (p: { meeting: MeetingRow }) => {
      repo.upsertMeeting(p.meeting);
      return { ok: true };
    },
    'memo.appendLive': (p: { meetingId: string; segment: Segment }) => {
      repo.appendSegment(p.meetingId, { ...p.segment, source: 'live' });
      return { ok: true };
    },
    'memo.finalize': async (p: FinalizeParams): Promise<MeetingSummary> => {
      const m = need(p.meetingId);
      repo.upsertMeeting({
        ...m,
        endedAt: p.endedAt,
        durationMs: p.durationMs,
        status: 'processing',
        ...(p.audioPath ? { audioPath: p.audioPath } : {}),
      });
      // The live streaming transcript is final (no re-transcription pass). Speaker names come
      // from fusing the AX active-speaker signal over each segment's time span.
      const segments = nameSegmentsFromAx(
        repo.getSegments(p.meetingId),
        p.active,
        p.durationMs,
      );
      repo.replaceSegments(p.meetingId, 'live', segments);
      const provider = createProvider(p.llm.provider, {
        fetch: deps.fetch,
        ...(p.llm.apiKey ? { apiKey: p.llm.apiKey } : {}),
        ...(p.llm.model ? { model: p.llm.model } : {}),
      });
      const summary = await summarizeMeeting(m.title, segments, provider, (e) =>
        deps.log?.('llm failed; using extractive fallback', { err: String(e) }),
      );
      repo.saveSummary(p.meetingId, summary);
      repo.setStatus(p.meetingId, 'done');
      return summary;
    },
    'memo.fail': (p: { meetingId: string }) => {
      repo.setStatus(p.meetingId, 'failed');
      return { ok: true };
    },
    'memo.list': (p: {
      query?: string;
    }): { meeting: MeetingRow; hit?: SearchHit }[] => {
      const q = p?.query?.trim();
      if (!q) return repo.listMeetings().map((meeting) => ({ meeting }));
      return repo.search(q).flatMap((hit) => {
        const meeting = repo.getMeeting(hit.meetingId);
        return meeting ? [{ meeting, hit }] : [];
      });
    },
    'memo.get': (p: { id: string }) => {
      const meeting = need(p.id);
      const summary = repo.getSummary(p.id);
      return {
        meeting,
        segments: repo.getSegments(p.id),
        ...(summary ? { summary } : {}),
      };
    },
    'memo.export': (p: { id: string }) => {
      const meeting = need(p.id);
      return {
        filename: exportFilename(meeting),
        markdown: meetingToMarkdown(
          meeting,
          repo.getSegments(p.id),
          repo.getSummary(p.id),
        ),
      };
    },
    'memo.delete': (p: { id: string }) => {
      repo.deleteMeeting(p.id);
      return { ok: true };
    },
  };
}

export type WorkerHandlers = ReturnType<typeof createWorkerHandlers>;

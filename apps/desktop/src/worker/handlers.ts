import fs from 'node:fs';
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
import { repairWav } from './wav-repair';

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
    /**
     * M5.1 crash recovery: meetings left in `recording` (app/helper died mid-meeting) or `processing` (died while
     * summarizing) are completed from what was persisted. The live transcript is final, so this is just the WAV
     * header repair plus the summary. Runs once at startup, before any new session can start.
     */
    'memo.recover': async (p: {
      llm: FinalizeParams['llm'];
    }): Promise<{ recovered: string[]; failed: string[] }> => {
      const recovered: string[] = [];
      const failed: string[] = [];
      const stuck = repo
        .listMeetings(1000)
        .filter((m) => m.status === 'recording' || m.status === 'processing');
      for (const m of stuck) {
        try {
          const segments = repo.getSegments(m.id);
          const audioMs = m.audioPath ? repairWav(m.audioPath) : undefined;
          const durationMs =
            m.durationMs ||
            audioMs ||
            Math.max(0, ...segments.map((s) => s.endMs));
          repo.upsertMeeting({
            ...m,
            durationMs,
            endedAt: m.endedAt ?? m.startedAt + durationMs,
            status: 'processing',
          });
          const provider = createProvider(p.llm.provider, {
            fetch: deps.fetch,
            ...(p.llm.apiKey ? { apiKey: p.llm.apiKey } : {}),
            ...(p.llm.model ? { model: p.llm.model } : {}),
          });
          const summary = await summarizeMeeting(
            m.title,
            segments,
            provider,
            (e) =>
              deps.log?.('llm failed; using extractive fallback', {
                err: String(e),
              }),
          );
          repo.saveSummary(m.id, summary);
          repo.setStatus(m.id, 'done');
          recovered.push(m.id);
        } catch (e) {
          deps.log?.('recover failed', { id: m.id, err: String(e) });
          repo.setStatus(m.id, 'failed');
          failed.push(m.id);
        }
      }
      return { recovered, failed };
    },
    /** M6.4 retention: delete audio of finished meetings older than `days` (transcripts and summaries stay). */
    'memo.pruneAudio': (p: { days: number; now?: number }) => {
      const pruned: string[] = [];
      if (!(p.days > 0)) return { pruned };
      const cutoff = (p.now ?? Date.now()) - p.days * 86_400_000;
      for (const m of repo.listMeetings(100_000)) {
        if (m.status !== 'done' || !m.audioPath) continue;
        if ((m.endedAt ?? m.startedAt) > cutoff) continue;
        try {
          fs.rmSync(m.audioPath, { force: true });
        } catch (e) {
          deps.log?.('prune failed', { id: m.id, err: String(e) });
          continue;
        }
        const { audioPath: _gone, ...rest } = m;
        repo.upsertMeeting(rest);
        pruned.push(m.id);
      }
      return { pruned };
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

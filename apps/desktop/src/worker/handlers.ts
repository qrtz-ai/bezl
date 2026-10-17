import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';
import {
  AppError,
  NoteRepo,
  createProvider,
  answerQuestion,
  blobToVec,
  dot,
  hybridRank,
  vecToBlob,
  type Embedder,
  type VecSource,
  type ChatAnswer,
  type ChatSource,
  type ChatTurn,
  exportFilename,
  nameSegmentsFromAx,
  meetingToMarkdown,
  summarizeMeeting,
  syntheticSegments,
  type ActiveSpeakerEvent,
  type FetchLike,
  type MeetingRow,
  type SearchHit,
  type MeetingSummary,
  type ProviderId,
  type Segment,
  type MeetingNote,
  getSchemaVersion,
  PipelineState,
  RecordingState,
  extractiveSummary,
  type PipelineContext,
} from '@qrtz/core';
import { dbHealth } from './db';
import { repairWav } from './wav-repair';

export interface LlmParams {
  provider: ProviderId | 'none';
  model?: string;
  apiKey?: string;
  /** Override for OpenAI-compatible endpoints (QRTZ_LLM_BASE_URL; tests use a local stub). */
  baseUrl?: string;
}

export interface FinalizeParams {
  meetingId: string;
  endedAt: number;
  durationMs: number;
  audioPath?: string;
  active: ActiveSpeakerEvent[];
  llm: LlmParams;
  /** false = skip the summary (Settings → Summarize after meetings off). */
  summarize?: boolean;
  /** The helper exited mid-recording; duration comes from the salvaged audio. */
  helperCrashed?: boolean;
}

/** All worker RPC methods. Pure over a DB + fetch so they're integration-testable in Node. */
export function createWorkerHandlers(
  db: DatabaseSync,
  deps: {
    fetch: FetchLike;
    log?: (msg: string, f?: object) => void;
    /** Lazy on-device embedder for semantic chat retrieval; null/throwing = full-text only (the fallback). */
    embedder?: () => Promise<Embedder | null>;
  },
) {
  const repo = new NoteRepo(db);
  let embedderBroken = false;
  const getEmbedder = async (): Promise<Embedder | null> => {
    if (!deps.embedder || embedderBroken) return null;
    try {
      return await deps.embedder();
    } catch (e) {
      embedderBroken = true;
      deps.log?.('embeddings unavailable; chat uses full-text retrieval', {
        err: String(e),
      });
      return null;
    }
  };
  /** Embed up to `cap` unindexed lines (one meeting or all), in batches. */
  const indexVectors = async (e: Embedder, cap: number, meetingId?: string) => {
    let done = 0;
    while (done < cap) {
      const batch = repo.missingVectors(
        e.id,
        Math.min(64, cap - done),
        meetingId,
      );
      if (!batch.length) break;
      const vecs = await e.embed(batch.map((b) => b.text));
      repo.saveVectors(
        e.id,
        batch.map((b, i) => ({
          rowid: b.rowid,
          meetingId: b.meetingId,
          vec: vecToBlob(vecs[i]!),
        })),
      );
      done += batch.length;
    }
    return done;
  };
  const summarize = (
    m: MeetingRow,
    segments: Segment[],
    llm: LlmParams,
    errMsg: string,
    strict = false,
  ) =>
    summarizeMeeting(
      m.title,
      segments,
      createProvider(llm.provider, {
        fetch: deps.fetch,
        ...(llm.apiKey ? { apiKey: llm.apiKey } : {}),
        ...(llm.model ? { model: llm.model } : {}),
        ...(llm.baseUrl ? { baseUrl: llm.baseUrl } : {}),
      }),
      (e) => deps.log?.(errMsg, { err: String(e) }),
      {
        notes: repo.getNotes(m.id),
        ...(m.template ? { template: m.template } : {}),
        strict,
      },
    );
  const need = (id: string) => {
    const m = repo.getMeeting(id);
    if (!m) throw new AppError('E_NOT_FOUND', `meeting ${id} not found`);
    return m;
  };
  return {
    // ---- durable recording lifecycle (docs/recording-lifecycle.md) ----
    'lifecycle.saveSession': (p: { state: RecordingState }) => {
      db.prepare(
        `INSERT INTO lifecycle_state (key,json,updated_at) VALUES ('session',?,?)
         ON CONFLICT(key) DO UPDATE SET json=excluded.json, updated_at=excluded.updated_at`,
      ).run(JSON.stringify(p.state), Date.now());
      return { ok: true };
    },
    'lifecycle.loadSession': (): RecordingState | null => {
      const r = db
        .prepare(`SELECT json FROM lifecycle_state WHERE key='session'`)
        .get() as { json: string } | undefined;
      if (!r) return null;
      const parsed = RecordingState.safeParse(JSON.parse(r.json));
      return parsed.success ? parsed.data : null;
    },
    'pipeline.save': (p: { state: PipelineState; settled: boolean }) => {
      if (!repo.getMeeting(p.state.meetingId)) return { ok: false }; // deleted meanwhile
      db.prepare(
        `INSERT INTO pipeline_state (meeting_id,json,settled,updated_at) VALUES (?,?,?,?)
         ON CONFLICT(meeting_id) DO UPDATE SET json=excluded.json, settled=excluded.settled, updated_at=excluded.updated_at`,
      ).run(
        p.state.meetingId,
        JSON.stringify(p.state),
        p.settled ? 1 : 0,
        p.state.updatedAt,
      );
      return { ok: true };
    },
    'pipeline.get': (p: { id: string }): PipelineState | null => {
      const r = db
        .prepare('SELECT json FROM pipeline_state WHERE meeting_id=?')
        .get(p.id) as { json: string } | undefined;
      if (!r) return null;
      const parsed = PipelineState.safeParse(JSON.parse(r.json));
      return parsed.success ? parsed.data : null;
    },
    'pipeline.listUnsettled': (): PipelineState[] =>
      (
        db
          .prepare(
            'SELECT json FROM pipeline_state WHERE settled=0 ORDER BY updated_at',
          )
          .all() as {
          json: string;
        }[]
      ).flatMap((r) => {
        const parsed = PipelineState.safeParse(JSON.parse(r.json));
        return parsed.success ? [parsed.data] : [];
      }),
    /** Meetings stuck in recording/processing that have no pipeline row (pre-lifecycle crashes). */
    'pipeline.orphans': (): MeetingRow[] =>
      repo
        .listByStatus(['recording', 'processing'])
        .filter(
          (m) =>
            !db
              .prepare('SELECT 1 FROM pipeline_state WHERE meeting_id=?')
              .get(m.id),
        ),
    /**
     * Step 1, idempotent: stitch WAV parts (one per helper restart) into the first part, repair the header,
     * fuse AX speaker names into the live segments, record duration. Safe to re-run after a crash at any point.
     */
    'note.stepFinalize': (p: { meetingId: string; ctx: PipelineContext }) => {
      const m = need(p.meetingId);
      const [main, ...rest] = p.ctx.audioParts;
      let audioMs: number | undefined;
      if (main) {
        repairWav(main);
        const extra = rest.filter((f) => fs.existsSync(f));
        if (extra.length && fs.existsSync(main)) {
          for (const f of extra) repairWav(f);
          const tmp = `${main}.stitch`;
          const out = fs.openSync(tmp, 'w');
          try {
            fs.writeSync(out, fs.readFileSync(main));
            for (const f of extra) {
              const b = fs.readFileSync(f);
              if (b.length > 44) fs.writeSync(out, b, 44, b.length - 44);
            }
          } finally {
            fs.closeSync(out);
          }
          fs.renameSync(tmp, main); // atomic: either the old main or the stitched one
          for (const f of extra) fs.rmSync(f, { force: true });
        }
        audioMs = repairWav(main);
      }
      const segs = repo.getSegments(p.meetingId);
      const durationMs =
        (p.ctx.helperCrashed && audioMs) ||
        p.ctx.durationMs ||
        audioMs ||
        Math.max(0, ...segs.map((s) => s.endMs));
      repo.upsertMeeting({
        ...m,
        endedAt: p.ctx.endedAt,
        durationMs,
        status: 'processing',
        ...(main && fs.existsSync(main) ? { audioPath: main } : {}),
      });
      if (p.ctx.active.length)
        repo.replaceSegments(
          p.meetingId,
          'live',
          nameSegmentsFromAx(segs, p.ctx.active, durationMs),
        );
      return { durationMs, segments: segs.length };
    },
    /**
     * Step 2. Strict: provider failures throw (AppError.retryable says whether a retry can help) and the
     * pipeline retries with backoff. With `fallback`, saves the on-device extractive summary + the LLM error.
     * Either way a settled summary marks the meeting done.
     */
    'note.stepSummary': async (p: {
      meetingId: string;
      llm: LlmParams;
      fallback?: { error: string };
      /** Auto-summarize is off: mark the meeting done without a summary. */
      skip?: boolean;
    }): Promise<MeetingSummary | null> => {
      const m = need(p.meetingId);
      if (p.skip) {
        repo.setStatus(p.meetingId, 'done');
        return null;
      }
      const segments = repo.getSegments(p.meetingId);
      const summary = p.fallback
        ? {
            ...extractiveSummary(segments, {
              notes: repo.getNotes(m.id),
              ...(m.template ? { template: m.template } : {}),
            }),
            llmError: p.fallback.error.slice(0, 500),
          }
        : await summarize(m, segments, p.llm, 'llm attempt failed', true);
      repo.saveSummary(p.meetingId, summary);
      repo.setStatus(p.meetingId, 'done');
      return summary;
    },
    /** Step 3: embed this meeting's lines for semantic chat. `skipped` = no embedder on this machine. */
    'note.stepIndex': async (p: { meetingId: string }) => {
      need(p.meetingId);
      const e = await getEmbedder();
      if (!e) return { indexed: 0, skipped: true };
      return {
        indexed: await indexVectors(e, 100_000, p.meetingId),
        skipped: false,
      };
    },
    'db.health': () => dbHealth(db),
    'note.create': (p: { meeting: MeetingRow }) => {
      repo.upsertMeeting(p.meeting);
      if (p.meeting.template)
        repo.setTemplate(p.meeting.id, p.meeting.template);
      return { ok: true };
    },
    'note.appendLive': (p: { meetingId: string; segment: Segment }) => {
      repo.appendSegment(p.meetingId, { ...p.segment, source: 'live' });
      return { ok: true };
    },
    'note.finalize': async (
      p: FinalizeParams,
    ): Promise<MeetingSummary | null> => {
      const m = need(p.meetingId);
      // Repairs the WAV header if the helper died before finalizing it; prefer the real audio length then.
      const audioMs = p.audioPath ? repairWav(p.audioPath) : undefined;
      const durationMs = p.helperCrashed && audioMs ? audioMs : p.durationMs;
      repo.upsertMeeting({
        ...m,
        endedAt: p.endedAt,
        durationMs,
        status: 'processing',
        ...(p.audioPath ? { audioPath: p.audioPath } : {}),
      });
      // The live streaming transcript is final (no re-transcription pass). Speaker names come
      // from fusing the AX active-speaker signal over each segment's time span.
      const segments = nameSegmentsFromAx(
        repo.getSegments(p.meetingId),
        p.active,
        durationMs,
      );
      repo.replaceSegments(p.meetingId, 'live', segments);
      if (p.summarize === false) {
        repo.setStatus(p.meetingId, 'done');
        return null;
      }
      const summary = await summarize(
        m,
        segments,
        p.llm,
        'llm failed; using extractive fallback',
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
    'note.recover': async (p: {
      llm: FinalizeParams['llm'];
    }): Promise<{ recovered: string[]; failed: string[] }> => {
      const recovered: string[] = [];
      const failed: string[] = [];
      const stuck = repo.listByStatus(['recording', 'processing']);
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
          const summary = await summarize(
            m,
            segments,
            p.llm,
            'llm failed; using extractive fallback',
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
    'note.pruneAudio': (p: { days: number; now?: number }) => {
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
    /** Dev/e2e only (main gates it on QRTZ_DEV_SEED and an unpackaged build): a long synthetic meeting. */
    /** Dev/e2e only: many short finished meetings (meeting-list paging/perf). */
    'dev.seedMany': (p: { count: number; now: number }) => {
      const segs = syntheticSegments(1);
      for (let i = 0; i < p.count; i++) {
        const id = `00000000-0000-4000-9000-${String(i).padStart(12, '0')}`;
        const startedAt = p.now - (i + 1) * 3_600_000;
        repo.upsertMeeting({
          id,
          title: `Sync #${p.count - i}`,
          platform: i % 2 ? 'zoom' : 'google-meet',
          startedAt,
          endedAt: startedAt + 60_000,
          durationMs: 60_000,
          status: 'done',
          participants: ['Ada Lovelace', 'Alan Turing'],
        });
        repo.replaceSegments(id, 'live', segs);
      }
      return { count: p.count };
    },
    'dev.seedLarge': async (p: {
      id: string;
      minutes: number;
      startedAt: number;
    }) => {
      const segments = syntheticSegments(p.minutes);
      const durationMs = p.minutes * 60_000;
      repo.upsertMeeting({
        id: p.id,
        title: `All-hands (${p.minutes} min)`,
        platform: 'google-meet',
        startedAt: p.startedAt,
        endedAt: p.startedAt + durationMs,
        durationMs,
        status: 'processing',
        participants: [...new Set(segments.map((s) => s.speakerName!))],
      });
      repo.replaceSegments(p.id, 'live', segments);
      const summary = await summarizeMeeting(
        `All-hands`,
        segments,
        null,
        () => {},
      );
      repo.saveSummary(p.id, summary);
      repo.setStatus(p.id, 'done');
      return { segments: segments.length };
    },
    'note.fail': (p: { meetingId: string }) => {
      repo.setStatus(p.meetingId, 'failed');
      return { ok: true };
    },
    /** Paged: callers ask for `limit` and get up to limit+1 rows; the extra row means "there's more". */
    'note.list': (p: {
      query?: string;
      limit?: number;
      offset?: number;
    }): { meeting: MeetingRow; hit?: SearchHit }[] => {
      const q = p?.query?.trim();
      if (!q)
        return repo
          .listMeetings((p?.limit ?? 100) + 1, p?.offset ?? 0)
          .map((meeting) => ({ meeting }));
      const off = p?.offset ?? 0;
      const lim = p?.limit ?? 100;
      return repo
        .search(q, off + lim + 1)
        .slice(off)
        .flatMap((hit) => {
          const meeting = repo.getMeeting(hit.meetingId);
          return meeting ? [{ meeting, hit }] : [];
        });
    },
    'note.get': (p: { id: string }) => {
      const meeting = need(p.id);
      const summary = repo.getSummary(p.id);
      return {
        meeting,
        segments: repo.getSegments(p.id),
        ...(summary ? { summary } : {}),
        notes: repo.getNotes(p.id),
      };
    },
    'note.export': (p: { id: string }) => {
      const meeting = need(p.id);
      return {
        filename: exportFilename(meeting),
        markdown: meetingToMarkdown(
          meeting,
          repo.getSegments(p.id),
          repo.getSummary(p.id),
          repo.getNotes(p.id),
        ),
      };
    },
    /** Re-run the summary (the "Retry" after an LLM failure). Keeps the old summary if this throws. */
    'note.resummarize': async (p: {
      id: string;
      llm: LlmParams;
      template?: string;
    }): Promise<MeetingSummary> => {
      const m = need(p.id);
      if (p.template) {
        repo.setTemplate(p.id, p.template);
        m.template = p.template;
      }
      const summary = await summarize(
        m,
        repo.getSegments(p.id),
        p.llm,
        'llm retry failed',
      );
      repo.saveSummary(p.id, summary);
      return summary;
    },
    'note.listNotes': (p: { meetingId: string }) => repo.getNotes(p.meetingId),
    'note.addNote': (p: { meetingId: string; text: string; atMs: number }) => {
      need(p.meetingId);
      const note: MeetingNote = {
        id: randomUUID(),
        atMs: Math.max(0, Math.round(p.atMs)),
        text: p.text,
        createdAt: Date.now(),
      };
      repo.addNote(p.meetingId, note);
      return { ...note, meetingId: p.meetingId };
    },
    'note.updateNote': (p: { id: string; text: string }) => ({
      ok: repo.updateNote(p.id, p.text),
    }),
    'note.deleteNote': (p: { id: string }) => ({ ok: repo.deleteNote(p.id) }),
    /** Consistent online snapshot of the whole DB (for backups). */
    'data.snapshot': (p: { path: string }) => {
      fs.rmSync(p.path, { force: true });
      db.prepare('VACUUM INTO ?').run(p.path);
      return {
        meetings: Number(
          (
            db.prepare('SELECT count(*) AS n FROM meetings').get() as {
              n: number;
            }
          ).n,
        ),
      };
    },
    /**
     * Merges a verified backup DB: meetings whose id isn't present are copied with their transcript, summary
     * and notes. Audio paths are rewritten through `audioMap` (backup file name → restored path).
     */
    'data.importDb': (p: {
      path: string;
      audioMap: Record<string, string>;
    }) => {
      const src = new DatabaseSync(p.path, { readOnly: true });
      try {
        const ok = (
          src.prepare('PRAGMA integrity_check').get() as {
            integrity_check: string;
          }
        ).integrity_check;
        if (ok !== 'ok')
          throw new AppError(
            'E_VALIDATION',
            `backup database is corrupt: ${ok}`,
          );
        const v = getSchemaVersion(src);
        if (v > getSchemaVersion(db))
          throw new AppError(
            'E_VALIDATION',
            `backup is from a newer qrtz (schema ${v})`,
          );
        const srcRepo = new NoteRepo(src);
        let imported = 0;
        let skipped = 0;
        const hasNotes = v >= 4;
        for (const m of srcRepo.listMeetings(1_000_000)) {
          if (repo.getMeeting(m.id)) {
            skipped++;
            continue;
          }
          const audio = m.audioPath
            ? p.audioMap[path.basename(m.audioPath)]
            : undefined;
          const { audioPath: _a, ...rest } = m;
          {
            // (replaceSegments/saveSummary open their own transactions; SQLite has no nested BEGIN.)
            repo.upsertMeeting({
              ...rest,
              ...(audio ? { audioPath: audio } : {}),
            });
            if (m.template) repo.setTemplate(m.id, m.template);
            const segs = srcRepo.getSegments(m.id);
            for (const kind of ['live', 'post'] as const) {
              const part = segs.filter((x) => x.source === kind);
              if (part.length) repo.replaceSegments(m.id, kind, part);
            }
            const sum = srcRepo.getSummary(m.id);
            if (sum) repo.saveSummary(m.id, sum);
            if (hasNotes)
              for (const n of srcRepo.getNotes(m.id)) repo.addNote(m.id, n);
          }
          imported++;
        }
        return { imported, skipped };
      } finally {
        src.close();
      }
    },
    /**
     * Grounded chat. With `meetingId`: retrieval over that meeting's transcript; otherwise FTS retrieval across all
     * meetings. LLM (OpenRouter by default) answers from the excerpts; without a key, extractive quotes.
     */
    'note.chat': async (p: {
      question: string;
      meetingId?: string;
      history?: ChatTurn[];
      llm: LlmParams;
    }): Promise<ChatAnswer> => {
      let candidates: ChatSource[];
      if (p.meetingId) {
        const m = need(p.meetingId);
        candidates = repo.getSegments(p.meetingId).map((s) => ({
          meetingId: m.id,
          title: m.title,
          startedAt: m.startedAt,
          startMs: s.startMs,
          ...(s.speakerName || s.speaker
            ? { speaker: (s.speakerName ?? s.speaker)! }
            : {}),
          text: s.text,
        }));
      } else candidates = repo.searchSegments(p.question, 80);

      // Hybrid (semantic + lexical) when the on-device embedder is available; otherwise lexical only.
      let ranked: ChatSource[] | null = null;
      const e = await getEmbedder();
      if (e) {
        try {
          await indexVectors(e, p.meetingId ? 5000 : 2000, p.meetingId);
          const [qvec] = await e.embed([p.question]);
          const indexed: VecSource[] = repo
            .vectorSources(e.id, p.meetingId)
            .map((r) => ({ ...r, vec: blobToVec(r.vec) }));
          const key = (s: ChatSource) =>
            `${s.meetingId}|${s.startMs}|${s.text}`;
          const byKey = new Map(indexed.map((s) => [key(s), s]));
          let pool: VecSource[] = candidates.map((c) => byKey.get(key(c)) ?? c);
          if (!p.meetingId) {
            // Across meetings: add the semantically closest lines that full-text search missed.
            const seen = new Set(pool.map(key));
            const extra = indexed
              .filter((s) => !seen.has(key(s)))
              .map((s) => ({ s, sim: dot(qvec!, s.vec!) }))
              .sort((a, b) => b.sim - a.sim)
              .slice(0, 40)
              .map((x) => x.s);
            pool = [...pool, ...extra];
          }
          ranked = hybridRank(p.question, pool, qvec!, 24);
        } catch (err) {
          deps.log?.('semantic retrieval failed; using full-text', {
            err: String(err),
          });
        }
      }
      const provider = createProvider(p.llm.provider, {
        fetch: deps.fetch,
        ...(p.llm.apiKey ? { apiKey: p.llm.apiKey } : {}),
        ...(p.llm.model ? { model: p.llm.model } : {}),
        ...(p.llm.baseUrl ? { baseUrl: p.llm.baseUrl } : {}),
      });
      return ranked
        ? answerQuestion(p.question, ranked, provider, p.history ?? [], true)
        : answerQuestion(p.question, candidates, provider, p.history ?? []);
    },
    /** Background: embed new transcript lines so chat retrieval is ready (no-op without an embedder). */
    'note.indexVectors': async (p: { cap?: number }) => {
      const e = await getEmbedder();
      return {
        indexed: e ? await indexVectors(e, p?.cap ?? 2000) : 0,
        model: e?.id ?? null,
      };
    },
    'note.renameSpeaker': (p: {
      meetingId: string;
      from: string;
      to: string;
    }) => {
      need(p.meetingId);
      return { changed: repo.renameSpeaker(p.meetingId, p.from, p.to) };
    },
    'note.delete': (p: { id: string }) => {
      repo.deleteMeeting(p.id);
      return { ok: true };
    },
  };
}

export type WorkerHandlers = ReturnType<typeof createWorkerHandlers>;

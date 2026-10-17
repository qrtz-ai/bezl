/**
 * Hardening cycle 14 (N49, ADR 0047): a chat question in Chinese, Japanese or Thai finds the lines about it.
 * unicode61 keeps a run of those scripts as one token and the question has no spaces, so retrieval and the ranker
 * used to treat the whole question as one word that no line starts with. spec/stress/chat-cjk.json is shared with
 * crates/qrtz-storage/tests/hardening14.rs (plans and retrieval); the ranks are TS until qrtz-search ports the ranker.
 * `QRTZ_UPDATE_SPEC=1` rewrites the expectations from this oracle (check the diff by hand).
 */
import fs from 'node:fs';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { migrate, migrations } from '../db/migrate';
import { rankSources, type ChatSource } from './chat';
import { CHAT_MAX_BIGRAMS, chatPlan, NoteRepo } from './repo';

interface Spec {
  plans: { input: string; fts?: string | null; substrings?: string[] | null }[];
  meetings: {
    id: string;
    title: string;
    startedAt: number;
    segments: { id: string; startMs: number; text: string; speaker?: string }[];
  }[];
  retrievals: { input: string; limit: number; hits?: [string, number][] }[];
  ranks: { question: string; k: number; texts?: string[] }[];
}
const here = path.dirname(fileURLToPath(import.meta.url));
const file = path.resolve(here, '../../../../spec/stress/chat-cjk.json');
const spec = JSON.parse(fs.readFileSync(file, 'utf8')) as Spec;
const update = process.env.QRTZ_UPDATE_SPEC === '1';

function seeded() {
  const db = new DatabaseSync(':memory:');
  migrate(db, migrations);
  const repo = new NoteRepo(db);
  for (const m of spec.meetings) {
    repo.upsertMeeting({
      id: m.id,
      title: m.title,
      platform: 'zoom',
      startedAt: m.startedAt,
      status: 'done',
      participants: [],
    });
    repo.replaceSegments(
      m.id,
      'live',
      m.segments.map((s) => ({
        id: s.id,
        startMs: s.startMs,
        endMs: s.startMs + 500,
        text: s.text,
        ...(s.speaker ? { speakerName: s.speaker } : {}),
        source: 'live' as const,
      })),
    );
  }
  return { db, repo };
}

const sources = (): ChatSource[] =>
  spec.meetings.flatMap((m) =>
    m.segments.map((s) => ({
      meetingId: m.id,
      title: m.title,
      startedAt: m.startedAt,
      startMs: s.startMs,
      ...(s.speaker ? { speaker: s.speaker } : {}),
      text: s.text,
    })),
  );

describe('hardening 14: chat in unsegmented scripts (N49)', () => {
  it('plans every spec question the same way as Rust', () => {
    for (const c of spec.plans) {
      const p = chatPlan(c.input);
      if (update) {
        c.fts = p ? p.fts : null;
        c.substrings = p ? p.substrings : null;
        continue;
      }
      expect(p && { fts: p.fts, substrings: p.substrings }, c.input).toEqual(
        c.substrings === null ? null : { fts: c.fts, substrings: c.substrings },
      );
      if (p) expect(p.substrings.length).toBeLessThanOrEqual(CHAT_MAX_BIGRAMS);
    }
  });

  it('retrieves the spec lines across meetings', () => {
    const { db, repo } = seeded();
    for (const c of spec.retrievals) {
      const got = repo
        .searchSegments(c.input, c.limit)
        .map((h) => [h.meetingId, h.startMs] as [string, number]);
      if (update) c.hits = got;
      else expect(got, c.input).toEqual(c.hits);
    }
    db.close();
  });

  it('ranks the lines about the question first', () => {
    const all = sources();
    for (const c of spec.ranks) {
      const got = rankSources(c.question, all, c.k).map((s) => s.text);
      if (update) c.texts = got;
      else expect(got, c.question).toEqual(c.texts);
    }
  });

  it('a Chinese question with no LLM quotes the line, not "nothing found"', () => {
    const { db, repo } = seeded();
    const hits = repo.searchSegments('我们决定了什么', 80);
    const top = rankSources('我们决定了什么', hits, 3);
    expect(top[0]?.text).toBe('我们决定推迟发布。');
    db.close();
  });

  it('a question without such a word keeps the indexed query', () => {
    expect(chatPlan('a ship it, Friday')).toEqual({
      fts: '"ship"* OR "it"* OR "friday"*',
      substrings: [],
    });
  });

  it('scans 200k lines in bounded time', { timeout: 30_000 }, () => {
    const db = new DatabaseSync(':memory:');
    migrate(db, migrations);
    const repo = new NoteRepo(db);
    repo.upsertMeeting({
      id: 'big',
      title: 'big',
      platform: 'zoom',
      startedAt: 1,
      status: 'done',
      participants: [],
    });
    const lines = [
      '我们讨论了一下进度',
      'the roadmap is on track',
      'เราจะคุยกันพรุ่งนี้',
      '来週また話しましょう',
    ];
    repo.replaceSegments(
      'big',
      'live',
      Array.from({ length: 200_000 }, (_, i) => ({
        id: `s${i}`,
        startMs: i * 1000,
        endMs: i * 1000 + 900,
        text:
          i === 123_456 ? '下周五之前把预算定下来' : lines[i % lines.length]!,
        source: 'live' as const,
      })),
    );
    const t0 = performance.now();
    const hits = repo.searchSegments(
      '我们下周五之前必须把预算和发布计划都定下来吧',
      80,
    );
    const ms = performance.now() - t0;
    expect(hits[0]?.text).toBe('下周五之前把预算定下来');
    console.log(`N49 scan 200k lines: ${ms.toFixed(0)} ms`);
    expect(ms).toBeLessThan(5_000);
    db.close();
  });
});

if (update)
  describe('write spec', () => {
    it('writes chat-cjk.json', () => {
      fs.writeFileSync(file, JSON.stringify(spec, null, 2) + '\n');
    });
  });

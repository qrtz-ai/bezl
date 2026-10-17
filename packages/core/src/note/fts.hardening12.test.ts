/**
 * Hardening cycle 12 (N48, ADR 0039): a word inside a run of Han, kana or Thai is found by search and the meeting
 * list. spec/stress/fts-substring.json is shared with crates/qrtz-storage/tests/hardening12.rs: the plan, the
 * snippet and the end-to-end hits must be the same on both runtimes.
 */
import fs from 'node:fs';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { migrate, migrations } from '../db/migrate';
import { NoteRepo, searchPlan, substringSnippet, toFtsQuery } from './repo';
import type { MeetingSummary } from './summary';

interface Spec {
  plans: { input: string; fts: string | null; substrings: string[] | null }[];
  snippets: { text: string; terms: string[]; snippet: string }[];
  meetings: {
    id: string;
    title: string;
    startedAt: number;
    segments: { id: string; startMs: number; text: string }[];
    summary?: string;
  }[];
  searches: {
    input: string;
    hits: { meetingId: string; snippet: string; startMs?: number }[];
  }[];
}
const here = path.dirname(fileURLToPath(import.meta.url));
const spec = JSON.parse(
  fs.readFileSync(
    path.resolve(here, '../../../../spec/stress/fts-substring.json'),
    'utf8',
  ),
) as Spec;

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
        ...s,
        endMs: s.startMs + 500,
        source: 'live' as const,
      })),
    );
    if (m.summary !== undefined) {
      const summary: MeetingSummary = {
        provider: 'test',
        summary: m.summary,
        actionItems: [],
        qa: [],
      };
      repo.saveSummary(m.id, summary, 1);
    }
  }
  return { db, repo };
}

describe('hardening 12: words inside unsegmented script runs (N48)', () => {
  it('plans every spec input the same way as Rust', () => {
    for (const c of spec.plans) {
      const p = searchPlan(c.input);
      expect(p && { fts: p.fts, substrings: p.substrings }, c.input).toEqual(
        c.substrings === null ? null : { fts: c.fts, substrings: c.substrings },
      );
    }
  });

  it('a plan with no substring term is exactly the indexed query (no behaviour change for spaced scripts)', () => {
    for (const c of spec.plans)
      if (c.substrings && !c.substrings.length)
        expect(c.fts, c.input).toBe(toFtsQuery(c.input));
  });

  it('builds the spec snippet', () => {
    for (const c of spec.snippets)
      expect(substringSnippet(c.text, c.terms), c.text).toBe(c.snippet);
  });

  it('search returns the spec hits end to end', () => {
    const { db, repo } = seeded();
    for (const c of spec.searches)
      expect(
        repo.search(c.input).map((h) => ({
          meetingId: h.meetingId,
          snippet: h.snippet,
          ...(h.startMs === undefined ? {} : { startMs: h.startMs }),
        })),
        c.input,
      ).toEqual(c.hits);
    db.close();
  });

  it('a deleted meeting is not found by substring', () => {
    const { db, repo } = seeded();
    expect(repo.search('会议').map((h) => h.meetingId)).toEqual(['zh']);
    repo.deleteMeeting('zh');
    expect(repo.search('会议')).toEqual([]);
    expect(repo.search('纪要')).toEqual([]);
    db.close();
  });

  it('stays fast on a large database: 200k lines, substring query under 1.5 s', () => {
    const db = new DatabaseSync(':memory:');
    migrate(db, migrations);
    const repo = new NoteRepo(db);
    for (let m = 0; m < 20; m++) {
      repo.upsertMeeting({
        id: `m${m}`,
        title: `Meeting ${m}`,
        platform: 'zoom',
        startedAt: m,
        status: 'done',
        participants: [],
      });
      repo.replaceSegments(
        `m${m}`,
        'live',
        Array.from({ length: 10_000 }, (_, i) => ({
          id: `s${i}`,
          startMs: i * 1000,
          endMs: i * 1000 + 900,
          text:
            i === 9_999 && m === 7
              ? '最后我们确认了会议纪要'
              : `line ${i} 我们讨论了产品路线图和招聘 roadmap hiring`,
          source: 'live' as const,
        })),
      );
    }
    const t = performance.now();
    const hits = repo.search('会议纪要');
    const ms = performance.now() - t;
    expect(hits.map((h) => [h.meetingId, h.startMs])).toEqual([
      ['m7', 9_999_000],
    ]);
    expect(ms).toBeLessThan(1500);
    db.close();
  }, 30_000); // seeding 200k lines takes ~5 s on the box; only the query is timed
});

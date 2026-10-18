import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  fields,
  logFiles,
  num,
  parse,
  pct,
  render,
  summarize,
} from './echo-timing-report.mjs';

// Lines as qrtz writes them: the helper's line through the host log (target `helper`), talk's through tracing's
// fmt layer. One talk line in the old `Some(…)` form (before ADR 0156) and one in the new.
const H = (rest) =>
  `2026-10-18T20:01:02.000Z  INFO helper: echo timing: ${rest}`;
const LOG = [
  H(
    'asr=parakeet-tdt-v2 audio_ms=4210 preroll_ms=300 first_partial_ms=-1 appends=49 append_total_ms=181.2 append_max_ms=17.4 live_rtf=0.043 tail_ms=51 drain_ms=118 close_ms=118 finalize_ms=146 release_to_stopped_ms=147 queued_ms=0 word_count=11 gate=tap gate_ducked=3/40 armed=1 segments=3 bg_segments=2 cap_cuts=0 tail_decode_ms=84.2 backlog_wait_ms=0.0 tail_conf=0.930',
  ),
  H(
    'asr=parakeet-tdt-v2 audio_ms=1200 preroll_ms=0 first_partial_ms=-1 appends=12 append_total_ms=40.0 append_max_ms=9.0 live_rtf=0.033 tail_ms=51 drain_ms=90 close_ms=90 finalize_ms=300 release_to_stopped_ms=310 queued_ms=0 word_count=1 gate=off-no-grant segments=1 bg_segments=0 cap_cuts=1 tail_decode_ms=210.0 backlog_wait_ms=12.5 tail_conf=0.410 tail_dropped=1',
  ),
  H(
    'asr=parakeet-tdt-v2 audio_ms=800 preroll_ms=0 first_partial_ms=-1 appends=8 append_total_ms=20.0 append_max_ms=5.0 live_rtf=0.025 tail_ms=51 drain_ms=-1 close_ms=-1 finalize_ms=-1 release_to_stopped_ms=60 queued_ms=0 word_count=0 drain_timed_out=1 segments=1 bg_segments=0 cap_cuts=0 tail_decode_ms=-1 backlog_wait_ms=0.0',
  ),
  '2026-10-18T20:01:02.100Z  INFO qrtz_talk::service: talk latency engine="on-device" audio_ms=4210.0 head_ms=0.0 first_partial_ms=Some(890.4) final_ms=Some(160.0) cleanup_ms=Some(1.0) inserted_ms=Some(175.0) insert=AxSelectedText cleanup=Rules raw_chars=58 inserted_chars=59 level_blocks=49 level_p10_dbfs=-61 level_p90_dbfs=-24',
  '2026-10-18T20:02:00.000Z  INFO qrtz_talk::service: talk latency engine="on-device" audio_ms=1200.0 head_ms=0.0 final_ms=320 inserted_ms=330 insert=Clipboard cleanup=Rules raw_chars=5 inserted_chars=5 level_blocks=12 level_p10_dbfs=-70 level_p90_dbfs=-48 tail_conf=0.41 tail_dropped=true',
  '2026-10-17T09:00:00.000Z  INFO qrtz_app: unrelated line',
].join('\n');

describe('echo-timing-report', () => {
  it('reads key=value fields after the marker, unquoting strings', () => {
    expect(
      fields('x echo timing: asr=a b_c=1 gate_ducked=3/40', 'echo timing:'),
    ).toEqual({ asr: 'a', b_c: '1', gate_ducked: '3/40' });
    expect(
      fields('talk latency engine="on-device" x=1', 'talk latency'),
    ).toEqual({ engine: 'on-device', x: '1' });
    expect(fields('no marker', 'echo timing:')).toBeNull();
  });

  it('treats -1 as a stage that did not run and unwraps the old Some(…) form', () => {
    expect(num('-1')).toBeUndefined();
    expect(num('Some(890.4)')).toBe(890.4);
    expect(num('None')).toBeUndefined();
    expect(num('-24', { negativeIsMissing: false })).toBe(-24);
    expect(num('abc')).toBeUndefined();
  });

  it('uses nearest-rank percentiles', () => {
    expect(pct([5, 1, 3, 2, 4], 50)).toBe(3);
    expect(pct([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 90)).toBe(9);
    expect(pct([], 50)).toBeUndefined();
  });

  it('summarizes stage times, flags, gate modes, tail confidence and mic level', () => {
    const s = summarize(parse(LOG));
    expect(s.dictations).toEqual({ helper: 3, talk: 2 });
    const p = s.byAsr['parakeet-tdt-v2'];
    expect(p.dictations).toBe(3);
    expect(p.times.release_to_stopped_ms).toEqual({
      n: 3,
      p50: 147,
      p90: 310,
      max: 310,
    });
    expect(p.times.first_partial_ms.n).toBe(0); // Parakeet sends no partials
    expect(p.times.finalize_ms.n).toBe(2); // -1 left out
    expect(p.times.tail_decode_ms).toEqual({
      n: 2,
      p50: 84.2,
      p90: 210,
      max: 210,
    });
    expect(p.flags).toMatchObject({
      drain_timed_out: 1,
      armed: 1,
      cap_cut: 1,
      tail_no_speech: 1,
    });
    expect(s.gate).toEqual({ tap: 1, 'off-no-grant': 1, none: 1 });
    expect(s.gateDucked).toEqual({ ducked: 3, blocks: 40 });
    expect(s.tailConf).toMatchObject({
      n: 2,
      belowFloor: 1,
      dropped: 1,
      p50: 0.41,
    });
    const t = s.byEngine['on-device'];
    expect(t.times.final_ms).toEqual({ n: 2, p50: 160, p90: 320, max: 320 });
    expect(t.times.first_partial_ms.n).toBe(1);
    expect(t.level_p90_dbfs.p50).toBe(-48);
    expect(t.quiet).toBe(1);
    expect(t.insert).toEqual({ AxSelectedText: 1, Clipboard: 1 });
    expect(t.tail_dropped).toBe(1);
    expect(s.notes.join('\n')).toMatch(/tail_decode_ms p90 210 ms is over 150/);
    expect(s.notes.join('\n')).toMatch(/gate=off-no-grant on 1/);
    expect(s.notes.join('\n')).toMatch(/too few to calibrate/);
  });

  it('drops lines dated before --since', () => {
    const s = summarize(
      parse(LOG.replace(/2026-10-18T20:02/, '2026-10-16T20:02'), {
        since: '2026-10-18',
      }),
    );
    expect(s.dictations).toEqual({ helper: 3, talk: 1 });
  });

  it('renders Markdown, and says so when there is nothing', () => {
    const md = render(summarize(parse(LOG)));
    expect(md).toContain('| release_to_stopped_ms | 3 | 147 | 310 | 310 |');
    expect(md).toContain('## release tail confidence');
    expect(render(summarize(parse('')))).toContain(
      'No dictation timing lines found.',
    );
  });

  it('reads every qrtz.log* in a directory, skipping other files', () => {
    const d = fs.mkdtempSync(path.join(os.tmpdir(), 'echo-report-'));
    fs.writeFileSync(path.join(d, 'qrtz.log'), '');
    fs.writeFileSync(path.join(d, 'qrtz.log.2026-10-17'), '');
    fs.writeFileSync(path.join(d, 'other.txt'), '');
    expect(logFiles([d]).map((p) => path.basename(p))).toEqual([
      'qrtz.log',
      'qrtz.log.2026-10-17',
    ]);
  });
});

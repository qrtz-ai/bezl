#!/usr/bin/env node
// Echo timing report (ADR 0156, docs/echo/dictation-tuning.md §5): turns the per-dictation timing lines in qrtz's
// local log into the numbers the tuning plan asks for. It reads two lines per dictation, both content-free by
// construction (ADR 0083 / 0099): the helper's `echo timing:` line (stage times, segment counts, gate mode, the
// release tail's confidence) and talk's `talk latency` line (final / insert times, mic level). Nothing leaves the
// Mac: it reads files and prints a Markdown summary (or `--json`).
//
// Usage: node scripts/echo-timing-report.mjs [--json] [--since YYYY-MM-DD] [log files or dirs …]
//        (default: ~/Library/Logs/qrtz, every qrtz.log* file in it)
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

/** The tail confidence floor ADR 0131 proposes (`QRTZ_ECHO_TAIL_CONF=0.6`). */
export const TAIL_FLOOR = 0.6;
/** §18: revisit the VAD floor only if the release tail decode's p90 tops this on the Mac. */
export const TAIL_DECODE_P90_LIMIT_MS = 150;
/** §10: below this speech level (p90 dBFS) a dictation counts as a quiet mic (rank 7). */
export const QUIET_P90_DBFS = -40;

/** key=value tokens after `marker`. Quoted values (talk's engine="on-device") lose their quotes. */
export function fields(line, marker) {
  const at = line.indexOf(marker);
  if (at < 0) return null;
  const out = {};
  const re = /([a-z_][a-z0-9_]*)=("(?:[^"\\]|\\.)*"|\S+)/g;
  for (const m of line.slice(at + marker.length).matchAll(re)) {
    let v = m[2];
    if (v.startsWith('"')) v = v.slice(1, -1);
    out[m[1]] = v;
  }
  return out;
}

/** A plain number, or undefined (`-1` = the stage didn't run; `Some(…)` from talk before ADR 0156 is unwrapped). */
export function num(v, { negativeIsMissing = true } = {}) {
  if (v === undefined) return undefined;
  const s = String(v).replace(/^Some\((.*)\)$/, '$1');
  if (s === 'None' || s === '') return undefined;
  const n = Number(s);
  if (!Number.isFinite(n)) return undefined;
  if (negativeIsMissing && n < 0) return undefined;
  return n;
}

const DATE = /^(\d{4}-\d{2}-\d{2})/;

/** The two kinds of record in a log text. `since` drops lines dated before it (lines without a date are kept). */
export function parse(text, { since } = {}) {
  const helper = [];
  const talk = [];
  for (const line of text.split(/\r?\n/)) {
    if (since) {
      const d = DATE.exec(line);
      if (d && d[1] < since) continue;
    }
    const h = fields(line, 'echo timing:');
    if (h) {
      helper.push(h);
      continue;
    }
    const t = fields(line, 'talk latency');
    if (t) talk.push(t);
  }
  return { helper, talk };
}

/** Nearest-rank percentile of a list (undefined when empty). */
export function pct(xs, p) {
  const v = xs.filter((x) => x !== undefined).sort((a, b) => a - b);
  if (!v.length) return undefined;
  const i = Math.min(
    v.length - 1,
    Math.max(0, Math.ceil((p / 100) * v.length) - 1),
  );
  return v[i];
}

function dist(rows, key, opts) {
  const xs = rows.map((r) => num(r[key], opts)).filter((x) => x !== undefined);
  return {
    n: xs.length,
    p50: pct(xs, 50),
    p90: pct(xs, 90),
    max: xs.length ? Math.max(...xs) : undefined,
  };
}

function count(rows, pred) {
  return rows.filter(pred).length;
}

function groupBy(rows, key, fallback) {
  const m = new Map();
  for (const r of rows) {
    const k = r[key] ?? fallback;
    if (!m.has(k)) m.set(k, []);
    m.get(k).push(r);
  }
  return m;
}

const HELPER_TIMES = [
  'release_to_stopped_ms',
  'first_partial_ms',
  'finalize_ms',
  'tail_decode_ms',
  'backlog_wait_ms',
  'preroll_ms',
  'audio_ms',
];
const TALK_TIMES = [
  'final_ms',
  'inserted_ms',
  'first_partial_ms',
  'head_ms',
  'cleanup_ms',
];

/** The report as data (what `--json` prints). */
export function summarize({ helper, talk }) {
  const byAsr = {};
  for (const [asr, rows] of groupBy(helper, 'asr', 'none')) {
    const times = {};
    for (const k of HELPER_TIMES) times[k] = dist(rows, k);
    times.live_rtf = dist(rows, 'live_rtf');
    byAsr[asr] = {
      dictations: rows.length,
      times,
      flags: {
        drain_timed_out: count(rows, (r) => r.drain_timed_out === '1'),
        final_timed_out: count(rows, (r) => r.final_timed_out === '1'),
        append_held: count(rows, (r) => r.append_held === '1'),
        decode_failures: count(rows, (r) => (num(r.decode_failures) ?? 0) > 0),
        armed: count(rows, (r) => r.armed === '1'),
        cap_cut: count(rows, (r) => (num(r.cap_cuts) ?? 0) > 0),
        tail_no_speech: count(rows, (r) => r.tail_decode_ms === '-1'),
      },
    };
  }

  const gate = {};
  let ducked = 0;
  let blocks = 0;
  for (const r of helper) {
    const mode = r.gate ?? (r.gate_ducked ? 'unlabelled' : 'none');
    gate[mode] = (gate[mode] ?? 0) + 1;
    const m = /^(\d+)\/(\d+)$/.exec(r.gate_ducked ?? '');
    if (m) {
      ducked += Number(m[1]);
      blocks += Number(m[2]);
    }
  }

  // The release tail's confidence: what ADR 0131's floor would keep or drop, on this Mac's Core ML numbers.
  const confs = helper
    .map((r) => num(r.tail_conf))
    .filter((x) => x !== undefined && x <= 1);
  const bins = [0.2, 0.4, 0.6, 0.8, 0.9, 1.0001].map((hi, i, a) => {
    const lo = i ? a[i - 1] : 0;
    return {
      lo,
      hi: Math.min(hi, 1),
      n: confs.filter((c) => c >= lo && c < hi).length,
    };
  });
  const tailConf = {
    n: confs.length,
    p5: pct(confs, 5),
    p10: pct(confs, 10),
    p50: pct(confs, 50),
    belowFloor: confs.filter((c) => c < TAIL_FLOOR).length,
    dropped: count(helper, (r) => r.tail_dropped === '1'),
    bins,
  };

  const byEngine = {};
  for (const [engine, rows] of groupBy(talk, 'engine', 'unknown')) {
    const times = {};
    for (const k of TALK_TIMES) times[k] = dist(rows, k);
    const p90s = rows.map((r) =>
      num(r.level_p90_dbfs, { negativeIsMissing: false }),
    );
    byEngine[engine] = {
      dictations: rows.length,
      times,
      level_p90_dbfs: dist(rows, 'level_p90_dbfs', {
        negativeIsMissing: false,
      }),
      quiet: p90s.filter((x) => x !== undefined && x < QUIET_P90_DBFS).length,
      saturated: count(rows, (r) => (num(r.level_saturated) ?? 0) > 0),
      insert: Object.fromEntries(
        Array.from(groupBy(rows, 'insert', 'none'), ([k, v]) => [k, v.length]),
      ),
      tail_dropped: count(rows, (r) => r.tail_dropped === 'true'),
    };
  }

  const notes = [];
  for (const [asr, a] of Object.entries(byAsr)) {
    const p90 = a.times.tail_decode_ms.p90;
    if (p90 !== undefined && p90 > TAIL_DECODE_P90_LIMIT_MS)
      notes.push(
        `${asr}: tail_decode_ms p90 ${p90} ms is over ${TAIL_DECODE_P90_LIMIT_MS} ms; revisit the VAD floor (§18).`,
      );
    if (a.flags.drain_timed_out || a.flags.final_timed_out)
      notes.push(
        `${asr}: ${a.flags.drain_timed_out} drain and ${a.flags.final_timed_out} final timeouts; the last words may be missing on those.`,
      );
    if (a.flags.decode_failures)
      notes.push(
        `${asr}: ${a.flags.decode_failures} dictations had a decode failure.`,
      );
  }
  const offGates = Object.entries(gate).filter(([k]) =>
    k.startsWith('off-no-'),
  );
  for (const [k, n] of offGates)
    notes.push(
      `gate=${k} on ${n} dictations: the speaker gate didn't run (ADR 0150).`,
    );
  if (tailConf.n >= 20)
    notes.push(
      `tail_conf: ${tailConf.belowFloor} of ${tailConf.n} release tails are under the ${TAIL_FLOOR} floor (p5 ${tailConf.p5}); compare with the Linux 70 / 122 phantom, 0 / 174 real (§17).`,
    );
  else if (tailConf.n)
    notes.push(
      `tail_conf: only ${tailConf.n} tails; too few to calibrate the floor (want 20+).`,
    );
  for (const [engine, e] of Object.entries(byEngine))
    if (e.quiet)
      notes.push(
        `${engine}: ${e.quiet} of ${e.dictations} dictations were a quiet mic (speech under ${QUIET_P90_DBFS} dBFS; rank 7).`,
      );

  return {
    dictations: { helper: helper.length, talk: talk.length },
    byAsr,
    gate,
    gateDucked: { ducked, blocks },
    tailConf,
    byEngine,
    notes,
  };
}

const f = (v) =>
  v === undefined
    ? '–'
    : Number.isInteger(v)
      ? String(v)
      : v.toFixed(v < 1 ? 3 : 1);

/** The report as Markdown, ready for docs/echo/dictation-tuning.md. */
export function render(s) {
  const out = [];
  out.push(
    `# echo timing report`,
    '',
    `${s.dictations.helper} helper lines, ${s.dictations.talk} talk lines.`,
    '',
  );
  for (const [asr, a] of Object.entries(s.byAsr)) {
    out.push(
      `## helper, asr=${asr} (${a.dictations})`,
      '',
      '| field | n | p50 | p90 | max |',
      '| --- | --- | --- | --- | --- |',
    );
    for (const [k, d] of Object.entries(a.times))
      if (d.n)
        out.push(`| ${k} | ${d.n} | ${f(d.p50)} | ${f(d.p90)} | ${f(d.max)} |`);
    out.push(
      '',
      'Flags: ' +
        Object.entries(a.flags)
          .map(([k, v]) => `${k} ${v}`)
          .join(', ') +
        '.',
      '',
    );
  }
  if (s.dictations.helper) {
    out.push(
      '## gate',
      '',
      Object.entries(s.gate)
        .map(([k, v]) => `${k} ${v}`)
        .join(', ') + '.',
    );
    if (s.gateDucked.blocks)
      out.push(
        `Mic blocks held down by the gate: ${s.gateDucked.ducked} / ${s.gateDucked.blocks}.`,
      );
    out.push('');
  }
  if (s.tailConf.n) {
    const t = s.tailConf;
    out.push(
      '## release tail confidence',
      '',
      `${t.n} tails, p5 ${f(t.p5)}, p10 ${f(t.p10)}, p50 ${f(t.p50)}; ${t.belowFloor} under ${TAIL_FLOOR}, ${t.dropped} dropped.`,
      '',
    );
    out.push('| conf | tails |', '| --- | --- |');
    for (const b of t.bins)
      out.push(`| ${b.lo.toFixed(1)}–${b.hi.toFixed(1)} | ${b.n} |`);
    out.push('');
  }
  for (const [engine, e] of Object.entries(s.byEngine)) {
    out.push(
      `## talk, engine=${engine} (${e.dictations})`,
      '',
      '| field | n | p50 | p90 | max |',
      '| --- | --- | --- | --- | --- |',
    );
    for (const [k, d] of Object.entries(e.times))
      if (d.n)
        out.push(`| ${k} | ${d.n} | ${f(d.p50)} | ${f(d.p90)} | ${f(d.max)} |`);
    const l = e.level_p90_dbfs;
    if (l.n)
      out.push(
        `| level_p90_dbfs | ${l.n} | ${f(l.p50)} | ${f(l.p90)} | ${f(l.max)} |`,
      );
    out.push(
      '',
      `Insert: ${Object.entries(e.insert)
        .map(([k, v]) => `${k} ${v}`)
        .join(
          ', ',
        )}. Quiet mic ${e.quiet}, saturated ${e.saturated}, tail dropped ${e.tail_dropped}.`,
      '',
    );
  }
  if (s.notes.length)
    out.push('## notes', '', ...s.notes.map((n) => `- ${n}`), '');
  if (!s.dictations.helper && !s.dictations.talk)
    out.push('No dictation timing lines found.', '');
  return out.join('\n');
}

/** Log files to read: each file argument, and every qrtz.log* in each directory argument. */
export function logFiles(args) {
  const files = [];
  for (const a of args) {
    const st = fs.statSync(a);
    if (st.isDirectory()) {
      for (const n of fs.readdirSync(a).sort())
        if (n.startsWith('qrtz.log')) files.push(path.join(a, n));
    } else files.push(a);
  }
  return files;
}

function main(argv) {
  const json = argv.includes('--json');
  const si = argv.indexOf('--since');
  const since = si >= 0 ? argv[si + 1] : undefined;
  const rest = argv.filter(
    (a, i) => !a.startsWith('--') && !(si >= 0 && i === si + 1),
  );
  const args = rest.length
    ? rest
    : [path.join(os.homedir(), 'Library', 'Logs', 'qrtz')];
  const all = { helper: [], talk: [] };
  for (const file of logFiles(args)) {
    const p = parse(fs.readFileSync(file, 'utf8'), { since });
    all.helper.push(...p.helper);
    all.talk.push(...p.talk);
  }
  const s = summarize(all);
  process.stdout.write(json ? JSON.stringify(s, null, 2) + '\n' : render(s));
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) ===
    path.resolve(new URL(import.meta.url).pathname)
)
  main(process.argv.slice(2));

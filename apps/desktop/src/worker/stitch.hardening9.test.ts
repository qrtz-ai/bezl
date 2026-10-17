/**
 * Hardening cycle 9 (H7, N44; ADR 0032): finalize pads each reconnect gap with zero PCM, from the part offsets on
 * the pipeline context, so audio time matches transcript time. Shared cases: spec/stress/stitch-gaps.json (Rust:
 * crates/qrtz-lifecycle/tests/hardening9.rs). Each part is filled with its own byte, so the test checks where every
 * part and every pad landed, not only the length.
 */
import { afterEach, describe, expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { STITCH_MAX_GAP_MS, gapPadBytes, stitchParts } from './wav-repair';

interface Case {
  name: string;
  parts: { dataBytes: number; missing?: boolean }[];
  offsetsMs?: number[];
  expect: {
    padBytes: (number | null)[];
    dataBytes: number;
    durationMs: number;
  };
}
const SPEC = JSON.parse(
  fs.readFileSync(
    new URL('../../../../spec/stress/stitch-gaps.json', import.meta.url),
    'utf8',
  ),
) as {
  format: { sampleRate: number; byteRate: number; blockAlign: number };
  maxGapMs: number;
  cases: Case[];
};

const dirs: string[] = [];
const tmp = () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-h9-stitch-'));
  dirs.push(d);
  return d;
};
afterEach(() => {
  for (const d of dirs.splice(0))
    fs.rmSync(d, { recursive: true, force: true });
});

/** A 44-byte-header 16-bit PCM WAV whose data is `fill` repeated; the header says 0 bytes (helper crashed). */
function writePart(file: string, dataBytes: number, fill: number) {
  const { sampleRate, byteRate, blockAlign } = SPEC.format;
  const b = Buffer.alloc(44 + dataBytes, fill);
  b.write('RIFF', 0);
  b.writeUInt32LE(36, 4);
  b.write('WAVE', 8);
  b.write('fmt ', 12);
  b.writeUInt32LE(16, 16);
  b.writeUInt16LE(1, 20);
  b.writeUInt16LE(1, 22);
  b.writeUInt32LE(sampleRate, 24);
  b.writeUInt32LE(byteRate, 28);
  b.writeUInt16LE(blockAlign, 32);
  b.writeUInt16LE(16, 34);
  b.write('data', 36);
  b.writeUInt32LE(0, 40);
  fs.writeFileSync(file, b);
}

const files = (d: string, n: number) =>
  Array.from({ length: n }, (_, i) =>
    path.join(d, i === 0 ? 'm.wav' : `m.p${i}.wav`),
  );

/** Runs of equal bytes in the data section: [byte, length][]. */
function runs(b: Buffer): [number, number][] {
  const out: [number, number][] = [];
  for (let i = 44; i < b.length;) {
    let j = i;
    while (j < b.length && b[j] === b[i]) j++;
    out.push([b[i]!, j - i]);
    i = j;
  }
  return out;
}

describe('hardening 9: stitched audio keeps meeting time across reconnect gaps (H7)', () => {
  it('the cap is the reconnect window plus the start ack, the same value as the spec', () => {
    expect(STITCH_MAX_GAP_MS).toBe(SPEC.maxGapMs);
  });

  for (const c of SPEC.cases)
    it(c.name, () => {
      const d = tmp();
      const parts = files(d, c.parts.length);
      c.parts.forEach((p, i) => {
        if (!p.missing) writePart(parts[i]!, p.dataBytes, i + 1);
      });
      // The pad plan, step by step, as the spec computes it.
      let have = c.parts[0]!.dataBytes;
      const pads = c.parts.slice(1).map((p, k) => {
        if (p.missing) return null;
        const pad =
          c.offsetsMs?.length === c.parts.length
            ? gapPadBytes(c.offsetsMs[k + 1]!, have, SPEC.format)
            : 0;
        have += pad + p.dataBytes;
        return pad;
      });
      expect(pads).toEqual(c.expect.padBytes);

      expect(stitchParts(parts, c.offsetsMs)).toBe(c.expect.durationMs);
      const out = fs.readFileSync(parts[0]!);
      expect(out.length).toBe(44 + c.expect.dataBytes);
      expect(out.readUInt32LE(40)).toBe(c.expect.dataBytes);
      expect(out.readUInt32LE(4)).toBe(36 + c.expect.dataBytes);
      // Layout: part 0, then for each later part its pad (zeros) and its bytes.
      const want: [number, number][] = [];
      const push = (v: number, n: number) => {
        if (!n) return;
        const last = want.at(-1);
        if (last && last[0] === v) last[1] += n;
        else want.push([v, n]);
      };
      push(1, c.parts[0]!.dataBytes);
      c.parts.slice(1).forEach((p, k) => {
        if (p.missing) return;
        push(0, c.expect.padBytes[k]!);
        push(k + 2, p.dataBytes);
      });
      expect(runs(out)).toEqual(want);
      for (const f of parts.slice(1)) expect(fs.existsSync(f)).toBe(false);
      expect(fs.readdirSync(d)).toEqual(['m.wav']);
      // Idempotent: a re-run (crash after the step, before it was recorded) changes nothing.
      expect(stitchParts(parts, c.offsetsMs)).toBe(c.expect.durationMs);
      expect(fs.readFileSync(parts[0]!).equals(out)).toBe(true);
    });

  it('disk full while stitching with offsets: the temp is removed, the first part intact, the retry pads', () => {
    const d = tmp();
    const [main, part] = files(d, 2) as [string, string];
    writePart(main, 32000, 1);
    fs.mkdirSync(part); // unreadable as a file: the copy fails after the temp was written
    const before = fs.readFileSync(main);
    expect(() => stitchParts([main, part], [0, 3000])).toThrow();
    expect(fs.readdirSync(d).sort()).toEqual(['m.p1.wav', 'm.wav']);
    // repairWav fixed the header in place (idempotent); the PCM is untouched.
    expect(fs.readFileSync(main).subarray(44).equals(before.subarray(44))).toBe(
      true,
    );
    fs.rmdirSync(part);
    writePart(part, 32000, 2);
    expect(stitchParts([main, part], [0, 3000])).toBe(4000);
    expect(runs(fs.readFileSync(main))).toEqual([
      [1, 32000],
      [0, 64000],
      [2, 32000],
    ]);
  });
});

import fs from 'node:fs';
import { FIXTURE_DURATION_MS, STANDUP_FIXTURE } from '@qrtz/core';

/** Writes a 16 kHz mono 16-bit WAV with tone bursts where the fixture speakers talk. */
export function writeFixtureWav(
  file: string,
  durationMs = FIXTURE_DURATION_MS,
  /** false = leave the header's sizes at 0, like a helper killed mid-recording. */
  finalized = true,
): void {
  const rate = 16_000;
  const n = Math.round((durationMs / 1000) * rate);
  const buf = Buffer.alloc(44 + n * 2);
  buf.write('RIFF', 0);
  buf.writeUInt32LE(36 + n * 2, 4);
  buf.write('WAVEfmt ', 8);
  buf.writeUInt32LE(16, 16);
  buf.writeUInt16LE(1, 20);
  buf.writeUInt16LE(1, 22);
  buf.writeUInt32LE(rate, 24);
  buf.writeUInt32LE(rate * 2, 28);
  buf.writeUInt16LE(2, 32);
  buf.writeUInt16LE(16, 34);
  buf.write('data', 36);
  buf.writeUInt32LE(n * 2, 40);
  const speakers = [...new Set(STANDUP_FIXTURE.lines.map((l) => l[0]))];
  for (const [who, start, end] of STANDUP_FIXTURE.lines) {
    const f = 180 + 70 * speakers.indexOf(who);
    for (
      let i = Math.round((start / 1000) * rate);
      i < Math.min(n, Math.round((end / 1000) * rate));
      i++
    ) {
      const env = 0.5 + 0.5 * Math.sin((2 * Math.PI * i) / (rate * 0.3));
      buf.writeInt16LE(
        Math.round(3000 * env * Math.sin((2 * Math.PI * f * i) / rate)),
        44 + i * 2,
      );
    }
  }
  if (!finalized) {
    buf.writeUInt32LE(36, 4);
    buf.writeUInt32LE(0, 40);
  }
  fs.writeFileSync(file, buf);
}

/**
 * Hardening cycle 5 (N28): a finalize that fails while stitching WAV parts (disk full, an unreadable part)
 * removes its meeting-sized `<main>.stitch` temp, keeps the first part intact and stays retryable; a delete
 * also removes a `.stitch` an older build left. Rust: crates/qrtz-lifecycle/tests/hardening5.rs.
 */
import { afterEach, describe, expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { openDatabase } from '../worker/db';
import { createWorkerHandlers } from '../worker/handlers';
import { meetingAudio } from './note-service';

const dirs: string[] = [];
const tmp = () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-h5-f-'));
  dirs.push(d);
  return d;
};
afterEach(() => {
  for (const d of dirs.splice(0))
    fs.rmSync(d, { recursive: true, force: true });
});
const wav = (file: string, dataBytes: number) => {
  const b = Buffer.alloc(44 + dataBytes);
  b.write('RIFF', 0);
  b.writeUInt32LE(36 + dataBytes, 4);
  b.write('WAVE', 8);
  b.write('fmt ', 12);
  b.writeUInt32LE(16, 16);
  b.writeUInt16LE(1, 20);
  b.writeUInt16LE(1, 22);
  b.writeUInt32LE(16000, 24);
  b.writeUInt32LE(32000, 28);
  b.writeUInt16LE(2, 32);
  b.writeUInt16LE(16, 34);
  b.write('data', 36);
  b.writeUInt32LE(dataBytes, 40);
  fs.writeFileSync(file, b);
};

describe('hardening 5: a failed stitch leaves no temp file (N28)', () => {
  it('removes <main>.stitch, keeps the first part byte for byte, and the step can be retried', () => {
    const d = tmp();
    const main = path.join(d, 'm.wav');
    const part = path.join(d, 'm.p1.wav');
    wav(main, 3200);
    fs.mkdirSync(part); // unreadable as a file: the read fails after the temp was opened
    const before = fs.readFileSync(main);
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: () => Promise.reject(new Error('offline')),
    });
    h['note.create']({
      meeting: {
        id: 'm',
        title: 'T',
        platform: 'zoom',
        startedAt: 1,
        status: 'processing',
        participants: [],
        audioPath: main,
      },
    });
    const ctx = {
      startedAt: 1,
      endedAt: 2,
      durationMs: 0,
      audioParts: [main, part],
      active: [],
      helperCrashed: true,
    };
    expect(() => h['note.stepFinalize']({ meetingId: 'm', ctx })).toThrow();
    expect(fs.existsSync(`${main}.stitch`)).toBe(false);
    expect(fs.readFileSync(main).equals(before)).toBe(true);
    // The part becomes readable (space freed): the retry stitches it.
    fs.rmdirSync(part);
    wav(part, 1600);
    const r = h['note.stepFinalize']({ meetingId: 'm', ctx });
    expect(r.durationMs).toBe(150);
    expect(fs.existsSync(part)).toBe(false);
  });

  it('a meeting delete also takes a leftover <id>.wav.stitch, never another meeting’s', () => {
    const d = tmp();
    for (const f of ['m.wav', 'm.wav.stitch', 'm.p2.wav', 'mm.wav.stitch'])
      fs.writeFileSync(path.join(d, f), '');
    const got = meetingAudio(d, 'm', [])
      .map((f) => path.basename(f))
      .sort();
    expect(got).toEqual(['m.p2.wav', 'm.wav', 'm.wav.stitch']);
  });
});

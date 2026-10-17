import fs from 'node:fs';

/**
 * Fixes the RIFF/data sizes of a 16-bit PCM WAV whose header was never finalized (helper crashed or was killed
 * mid-recording; both the Swift and mock helpers write a 0-length header first). Returns the duration in ms, or
 * undefined if the file is missing or not a canonical 44-byte-header WAV.
 */
export function repairWav(path: string): number | undefined {
  let fd: number;
  try {
    fd = fs.openSync(path, 'r+');
  } catch {
    return undefined;
  }
  try {
    const size = fs.fstatSync(fd).size;
    if (size < 44) return undefined;
    const h = Buffer.alloc(44);
    fs.readSync(fd, h, 0, 44, 0);
    if (
      h.toString('ascii', 0, 4) !== 'RIFF' ||
      h.toString('ascii', 8, 12) !== 'WAVE' ||
      h.toString('ascii', 36, 40) !== 'data'
    )
      return undefined;
    const dataBytes = size - 44;
    if (h.readUInt32LE(40) !== dataBytes) {
      h.writeUInt32LE(36 + dataBytes, 4);
      h.writeUInt32LE(dataBytes, 40);
      fs.writeSync(fd, h, 0, 44, 0);
    }
    const byteRate = h.readUInt32LE(28);
    return byteRate ? Math.round((dataBytes / byteRate) * 1000) : undefined;
  } finally {
    fs.closeSync(fd);
  }
}

/**
 * H7 (ADR 0032): the most silence one reconnect gap can add before a part: the reconnect window plus the start ack
 * (`RETRY_POLICIES.helperReconnectWindowMs + startAckMs`). A gap is never longer, so this only bounds a wall clock
 * that jumped forward while the helper was down. Same value in Rust (`wav::STITCH_MAX_GAP_MS`).
 */
export const STITCH_MAX_GAP_MS = 35_000;
/** Copy and padding block: constant memory however long the meeting (N44). */
const CHUNK = 1 << 20;

/** Byte rate and block align of a canonical 44-byte-header PCM WAV, or undefined. */
export function wavFormat(
  path: string,
): { byteRate: number; blockAlign: number } | undefined {
  let fd: number;
  try {
    fd = fs.openSync(path, 'r');
  } catch {
    return undefined;
  }
  try {
    const h = Buffer.alloc(44);
    if (fs.readSync(fd, h, 0, 44, 0) < 44) return undefined;
    if (
      h.toString('ascii', 0, 4) !== 'RIFF' ||
      h.toString('ascii', 8, 12) !== 'WAVE' ||
      h.toString('ascii', 36, 40) !== 'data'
    )
      return undefined;
    const byteRate = h.readUInt32LE(28);
    const blockAlign = h.readUInt16LE(32);
    return byteRate && blockAlign ? { byteRate, blockAlign } : undefined;
  } finally {
    fs.closeSync(fd);
  }
}

/**
 * H7: bytes of zero PCM that put a part recorded from meeting time `offsetMs` at its place, when `haveBytes` of
 * audio data precede it. Block-aligned, never negative (a part that overlaps: no pad), at most `STITCH_MAX_GAP_MS`.
 * Integer arithmetic on non-negative values, so Rust (`wav::gap_pad_bytes`) gets the same bytes.
 */
export function gapPadBytes(
  offsetMs: number,
  haveBytes: number,
  f: { byteRate: number; blockAlign: number },
): number {
  const at = (ms: number) =>
    Math.floor(
      (Math.max(0, Math.round(ms)) * f.byteRate) / (1000 * f.blockAlign),
    ) * f.blockAlign;
  return Math.max(0, Math.min(at(STITCH_MAX_GAP_MS), at(offsetMs) - haveBytes));
}

function writeZeros(fd: number, n: number, pos: number): number {
  const z = Buffer.alloc(Math.min(CHUNK, n));
  for (let left = n; left > 0;) {
    const k = Math.min(z.length, left);
    fs.writeSync(fd, z, 0, k, pos);
    pos += k;
    left -= k;
  }
  return pos;
}

/** Appends `src`'s PCM (its 44-byte header skipped) at `pos` in chunks; returns the new end. */
function appendPcm(out: number, src: string, pos: number): number {
  const fd = fs.openSync(src, 'r');
  try {
    const b = Buffer.alloc(CHUNK);
    for (let at = 44; ;) {
      const n = fs.readSync(fd, b, 0, b.length, at);
      if (n <= 0) return pos;
      fs.writeSync(out, b, 0, n, pos);
      pos += n;
      at += n;
    }
  } finally {
    fs.closeSync(fd);
  }
}

/**
 * Finalize's audio half (`note.stepFinalize`; Rust `wav::stitch_parts`): repair the first part, append every
 * existing later part's PCM via `<main>.stitch` + atomic rename, delete the parts, repair again. Returns the final
 * duration in ms.
 *
 * H7: with `offsetsMs` (one per part, meeting time, `PipelineContext.partOffsetsMs`) each later part is preceded by
 * the zero PCM of its reconnect gap (`gapPadBytes`), so audio time matches transcript time after a reconnect. A
 * context without offsets (older rows) stitches back to back, as before.
 *
 * N44: streamed (copy + 1 MiB chunks), so a three-hour meeting never sits in the worker's memory.
 * N28: a failed stitch (disk full, unreadable part) removes its temp and keeps the first part intact.
 */
export function stitchParts(
  parts: readonly string[],
  offsetsMs?: readonly number[],
): number | undefined {
  const [main, ...rest] = parts;
  if (!main) return undefined;
  repairWav(main);
  const offsets = offsetsMs?.length === parts.length ? offsetsMs : undefined;
  const extra = rest
    .map((f, i) => ({ f, offsetMs: offsets?.[i + 1] }))
    .filter((x) => fs.existsSync(x.f));
  if (extra.length && fs.existsSync(main)) {
    for (const x of extra) repairWav(x.f);
    const fmt = offsets ? wavFormat(main) : undefined;
    const tmp = `${main}.stitch`;
    try {
      fs.copyFileSync(main, tmp);
      const out = fs.openSync(tmp, 'r+');
      try {
        let pos = fs.fstatSync(out).size;
        for (const x of extra) {
          if (fmt && x.offsetMs !== undefined)
            pos = writeZeros(
              out,
              gapPadBytes(x.offsetMs, Math.max(0, pos - 44), fmt),
              pos,
            );
          pos = appendPcm(out, x.f, pos);
        }
      } finally {
        fs.closeSync(out);
      }
      fs.renameSync(tmp, main); // atomic: either the old main or the stitched one
    } catch (e) {
      // N28: a failed stitch (disk full, unreadable part) must not leave a meeting-sized `.stitch` behind.
      fs.rmSync(tmp, { force: true });
      throw e;
    }
    for (const x of extra) fs.rmSync(x.f, { force: true });
  }
  return repairWav(main);
}

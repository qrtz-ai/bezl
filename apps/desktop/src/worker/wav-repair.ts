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

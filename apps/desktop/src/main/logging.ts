import fs from 'node:fs';
import path from 'node:path';
import {
  createLogger,
  formatJsonLine,
  type LogLevel,
  type Logger,
  type LogRecord,
} from '@qrtz/core';

const MAX_BYTES = 5 * 1024 * 1024;

/** JSON-lines file sink with single-file rotation (main.jsonl → main.1.jsonl). */
export function createFileLogger(
  dir: string,
  level: LogLevel,
  echo: boolean,
): Logger {
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, 'main.jsonl');
  let size = fs.existsSync(file) ? fs.statSync(file).size : 0;
  const sink = (r: LogRecord) => {
    const line = formatJsonLine(r);
    if (size + line.length > MAX_BYTES) {
      try {
        fs.renameSync(file, path.join(dir, 'main.1.jsonl'));
      } catch {
        /* ignore */
      }
      size = 0;
    }
    size += line.length;
    fs.appendFile(file, line, () => {});
    if (echo) process.stdout.write(line);
  };
  return createLogger({ sink, level });
}

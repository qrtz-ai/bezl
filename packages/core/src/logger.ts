export type LogLevel = 'debug' | 'info' | 'warn' | 'error';
const ORDER: Record<LogLevel, number> = {
  debug: 10,
  info: 20,
  warn: 30,
  error: 40,
};

export interface LogRecord {
  ts: string;
  level: LogLevel;
  scope: string;
  msg: string;
  [field: string]: unknown;
}

export type LogSink = (record: LogRecord) => void;

export interface Logger {
  debug(msg: string, fields?: Record<string, unknown>): void;
  info(msg: string, fields?: Record<string, unknown>): void;
  warn(msg: string, fields?: Record<string, unknown>): void;
  error(msg: string, fields?: Record<string, unknown>): void;
  child(scope: string): Logger;
}

const SECRET_KEY = /(key|token|secret|password|authorization)/i;

/** Redacts secret-looking fields and serialises Errors. */
export function sanitizeFields(
  fields: Record<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(fields)) {
    if (SECRET_KEY.test(k)) out[k] = '[redacted]';
    else if (v instanceof Error)
      out[k] = { name: v.name, message: v.message, stack: v.stack };
    else out[k] = v;
  }
  return out;
}

export function createLogger(opts: {
  sink: LogSink;
  level?: LogLevel;
  scope?: string;
  now?: () => Date;
}): Logger {
  const min = ORDER[opts.level ?? 'info'];
  const now = opts.now ?? (() => new Date());
  const make = (scope: string): Logger => {
    const log =
      (level: LogLevel) =>
      (msg: string, fields: Record<string, unknown> = {}) => {
        if (ORDER[level] < min) return;
        opts.sink({
          ...sanitizeFields(fields),
          ts: now().toISOString(),
          level,
          scope,
          msg,
        });
      };
    return {
      debug: log('debug'),
      info: log('info'),
      warn: log('warn'),
      error: log('error'),
      child: (s) => make(scope ? `${scope}.${s}` : s),
    };
  };
  return make(opts.scope ?? 'qrtz');
}

export const formatJsonLine = (r: LogRecord): string =>
  JSON.stringify(r) + '\n';

export const nullLogger: Logger = createLogger({ sink: () => {} });

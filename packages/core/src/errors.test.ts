import { describe, expect, it } from 'vitest';
import { AppError, AppErrorJSON } from './errors';
import { createLogger, type LogRecord } from './logger';

describe('AppError', () => {
  it('round-trips through JSON', () => {
    const e = new AppError('E_TIMEOUT', 'slow', { details: { ms: 5 } });
    const j = AppErrorJSON.parse(JSON.parse(JSON.stringify(e.toJSON())));
    const back = AppError.fromJSON(j);
    expect(back.code).toBe('E_TIMEOUT');
    expect(back.retryable).toBe(true);
    expect(back.options.details).toEqual({ ms: 5 });
  });
  it('normalises unknown throws', () => {
    expect(AppError.from(new Error('x')).code).toBe('E_INTERNAL');
    expect(AppError.from('y').message).toBe('y');
  });
});

describe('logger', () => {
  it('filters by level, scopes children and redacts secrets', () => {
    const out: LogRecord[] = [];
    const log = createLogger({
      sink: (r) => out.push(r),
      level: 'info',
      now: () => new Date(0),
    });
    log.debug('hidden');
    log.child('ipc').info('hello', { apiKey: 'sk-123', n: 1 });
    expect(out).toEqual([
      {
        ts: '1970-01-01T00:00:00.000Z',
        level: 'info',
        scope: 'qrtz.ipc',
        msg: 'hello',
        apiKey: '[redacted]',
        n: 1,
      },
    ]);
  });
});

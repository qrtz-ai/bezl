// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { AppError } from '@qrtz/core';
import {
  RENDERER_LOG_PER_MINUTE,
  describeError,
  describeMediaError,
  installRendererLogging,
  scrub,
} from './rendererLog';

const SENTINEL = 'the quarterly zebra launch moves to thursday';

function sink() {
  const calls: { level: string; scope: string; message: string }[] = [];
  return {
    calls,
    invoke: async (_c: 'log:write', req: (typeof calls)[number]) => {
      calls.push(req);
      return { accepted: true };
    },
  };
}

describe('renderer log (ADR 0083)', () => {
  it('replaces long quoted values by their length', () => {
    expect(scrub(`Unexpected token in "${SENTINEL}"`)).toBe(
      `Unexpected token in "[${SENTINEL.length} chars]"`,
    );
    expect(scrub("bad enum 'mic'")).toBe("bad enum 'mic'");
    expect(scrub('x'.repeat(400)).length).toBe(301);
  });

  it('describes errors by name, message and bundle positions only', () => {
    const e = new TypeError(`cannot read "${SENTINEL}"`);
    e.stack = `TypeError: x\n    at f (tauri://localhost/assets/index-abc.js:10:5)\n    at g (tauri://localhost/assets/index-abc.js:20:7)`;
    const d = describeError(e);
    expect(d).toContain('TypeError: cannot read "[');
    expect(d).toContain('assets/index-abc.js:10:5');
    expect(d).not.toContain('tauri://');
    expect(d).not.toContain(SENTINEL);
    expect(describeError(new AppError('E_UNAVAILABLE', 'helper down'))).toBe(
      'E_UNAVAILABLE: helper down',
    );
    expect(describeError({ text: SENTINEL })).toBe(
      'non-error rejection (object)',
    );
  });

  it('logs the player error without the file name', () => {
    const a = document.createElement('audio');
    a.src = 'qrtz-media://rec/2026-10-17-weekly-sync.wav';
    const d = describeMediaError(a);
    expect(d).toMatch(/^audio error 0 /);
    expect(d).toContain('src qrtz-media:');
    expect(d).not.toContain('weekly');
  });

  it('sends media errors, uncaught errors and rejections, rate-limited', async () => {
    const s = sink();
    let t = 0;
    const off = installRendererLogging(s, 'main', window, () => t);
    const a = document.createElement('audio');
    document.body.appendChild(a);
    a.dispatchEvent(new Event('error'));
    window.dispatchEvent(
      new ErrorEvent('error', { message: 'boom', error: new Error('boom') }),
    );
    const rej = new Event('unhandledrejection') as PromiseRejectionEvent;
    Object.defineProperty(rej, 'reason', { value: new Error('nope') });
    window.dispatchEvent(rej);
    await Promise.resolve();
    expect(s.calls.map((c) => c.scope)).toEqual([
      'player',
      'main.uncaught',
      'main.rejection',
    ]);
    expect(s.calls[0]!.level).toBe('error');
    for (let i = 0; i < 100; i++) {
      t += 1;
      window.dispatchEvent(
        new ErrorEvent('error', {
          message: `e${i}`,
          error: new Error(`e${i}`),
        }),
      );
    }
    expect(s.calls.length).toBe(RENDERER_LOG_PER_MINUTE);
    t += 60_000;
    window.dispatchEvent(
      new ErrorEvent('error', { message: 'later', error: new Error('later') }),
    );
    expect(s.calls.length).toBe(RENDERER_LOG_PER_MINUTE + 1);
    off();
    window.dispatchEvent(new ErrorEvent('error', { message: 'gone' }));
    expect(s.calls.length).toBe(RENDERER_LOG_PER_MINUTE + 1);
  });
});

import { describe, expect, it } from 'vitest';
import { SessionView } from '@qrtz/core';
import { trayState } from './tray-model';

const v = (p: Partial<SessionView>) =>
  SessionView.parse({ phase: 'idle', ...p });

describe('tray state', () => {
  it('idle: start + open + quit', () => {
    const s = trayState(v({}));
    expect(s.icon).toBe('idle');
    expect(s.items.filter((i) => i.action).map((i) => i.action)).toEqual([
      'start',
      'open',
      'quit',
    ]);
  });
  it('recording: red icon, elapsed title, stop', () => {
    const s = trayState(
      v({ phase: 'recording', startedAt: 1000 }),
      1000 + 65_000,
    );
    expect(s.icon).toBe('rec');
    expect(s.title).toBe('1:05');
    expect(s.items.some((i) => i.action === 'stop')).toBe(true);
  });
  it('detected: record this meeting / not now', () => {
    const s = trayState(
      v({
        phase: 'detected',
        detected: {
          key: 'k',
          title: 'Standup',
          platform: 'zoom',
          app: 'zoom.us',
        },
      }),
    );
    expect(s.items.map((i) => i.label)).toContain('record this meeting');
    expect(s.items.some((i) => i.action === 'dismiss')).toBe(true);
  });
  it('helper missing disables start', () => {
    const s = trayState(v({ helper: 'missing' }));
    expect(s.items.find((i) => i.action === 'start')?.enabled).toBe(false);
  });
});

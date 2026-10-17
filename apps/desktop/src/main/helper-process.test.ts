import { describe, expect, it, vi } from 'vitest';

vi.mock('electron', () => ({ app: { isPackaged: false } }));
const { parseRevoke, parseSilence } = await import('./helper-process');

describe('mock helper fault-injection env', () => {
  it('QRTZ_MOCK_SILENT: source@silentMs[-audibleMs], junk dropped (N29)', () => {
    expect(parseSilence('system@1500, mic@2000-4000,camera@1,mic@x')).toEqual([
      { source: 'system', afterMs: 1500 },
      { source: 'mic', afterMs: 2000, audibleAfterMs: 4000 },
    ]);
  });
  it('QRTZ_MOCK_REVOKE still parses', () => {
    expect(parseRevoke('microphone@400')).toEqual([
      { permission: 'microphone', afterMs: 400 },
    ]);
  });
});

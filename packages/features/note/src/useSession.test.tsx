// @vitest-environment jsdom
/** N155: a note:session snapshot that answers after a newer event must not overwrite it. */
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { SessionView, sessionIsNewer, type QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { useSession } from './useRecording';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const view = (phase: string, seq?: number) =>
  SessionView.parse({ phase, ...(seq ? { seq } : {}) });

function Phase() {
  return <span data-testid="phase">{useSession().phase}</span>;
}

describe('useSession (N155)', () => {
  it('an older snapshot arriving after a newer event is ignored', async () => {
    let answer: (v: SessionView) => void = () => {};
    let emit: (v: SessionView) => void = () => {};
    const client = {
      subApp: 'note',
      invoke: () => new Promise<SessionView>((r) => (answer = r)),
      on: (_: string, cb: (v: SessionView) => void) => {
        emit = cb;
        return () => {};
      },
    } as unknown as QrtzClient;
    const el = document.createElement('div');
    const root = createRoot(el);
    await act(async () =>
      root.render(
        <QrtzClientProvider client={client}>
          <Phase />
        </QrtzClientProvider>,
      ),
    );
    // The meeting stops while the snapshot (taken before, still "recording") is on its way.
    await act(async () => emit(view('idle', 8)));
    await act(async () => answer(view('recording', 7)));
    expect(el.textContent).toBe('idle');
    // A newer event still applies.
    await act(async () => emit(view('recording', 9)));
    expect(el.textContent).toBe('recording');
    act(() => root.unmount());
  });

  it('views without a seq (old builds, mocks) always apply', () => {
    expect(sessionIsNewer(view('recording', 5), view('idle'))).toBe(true);
    expect(sessionIsNewer(null, view('idle', 1))).toBe(true);
    expect(sessionIsNewer(view('idle', 5), view('recording', 4))).toBe(false);
  });
});

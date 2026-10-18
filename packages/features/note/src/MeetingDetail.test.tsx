// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act, useState } from 'react';
import { createRoot } from 'react-dom/client';
import type { MeetingTab, QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import {
  SAMPLE_MEETING_ID,
  idleSession,
  ipcSamples,
} from '@qrtz/core/ipc/samples';
import { ThemeProvider } from '@qrtz/ui';
import { MeetingDetail } from './MeetingDetail';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

// The note view owns the tab through its route; a plain state stands in for it here.
function Routed() {
  const [tab, setTab] = useState<MeetingTab>('transcript');
  return (
    <MeetingDetail
      id={SAMPLE_MEETING_ID}
      tab={tab}
      onTab={setTab}
      onDelete={() => {}}
    />
  );
}

describe('MeetingDetail tabs', () => {
  it('switching transcript/summary/notes keeps panels mounted: no refetch, no empty frame', async () => {
    const calls: string[] = [];
    const client = {
      subApp: 'note',
      invoke: async (channel: string) => {
        calls.push(channel);
        if (channel === 'note:session') return idleSession;
        if (channel === 'note:list-notes')
          return ipcSamples['note:get'].response.notes;
        return (ipcSamples as Record<string, { response: unknown }>)[channel]
          ?.response;
      },
      on: () => () => {},
    } as unknown as QrtzClient;
    const el = document.createElement('div');
    document.body.appendChild(el);
    const root = createRoot(el);
    await act(async () =>
      root.render(
        <ThemeProvider mode="light">
          <QrtzClientProvider client={client}>
            <Routed />
          </QrtzClientProvider>
        </ThemeProvider>,
      ),
    );
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    const click = (t: string) =>
      act(() =>
        el.querySelector<HTMLElement>(`[data-testid="tab-${t}"]`)!.click(),
      );
    const shown = () =>
      [...el.querySelectorAll<HTMLElement>('[role="tabpanel"]')]
        .filter((p) => !p.hidden)
        .map((p) => p.dataset.panel);
    expect(shown()).toEqual(['transcript']);
    await act(async () => click('notes'));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    const noteLoads = calls.filter((c) => c === 'note:list-notes').length;
    const gets = calls.filter((c) => c === 'note:get').length;
    expect(noteLoads).toBe(1);
    for (const t of ['summary', 'transcript', 'notes', 'summary']) {
      click(t);
      // The panel is on screen in the same commit as the click.
      expect(shown()).toEqual([t]);
      expect(
        el.querySelector(`[data-panel="${t}"]`)!.textContent!.length,
      ).toBeGreaterThan(0);
    }
    click('notes');
    expect(calls.filter((c) => c === 'note:list-notes').length).toBe(noteLoads);
    expect(calls.filter((c) => c === 'note:get').length).toBe(gets);
    act(() => root.unmount());
  });
});

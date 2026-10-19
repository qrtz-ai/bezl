// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ThemeProvider } from '@qrtz/ui';
import { CalendarSettings, Upcoming } from './Calendar';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const now = Date.now();
const status = (access: string, enabled: boolean) => ({
  enabled,
  access,
  calendars:
    access === 'granted'
      ? [
          { id: 'w', title: 'Work' },
          { id: 'h', title: 'Home' },
        ]
      : [],
  chosen: null,
  lastSyncAt: null,
});

function mount(
  ui: React.ReactNode,
  answers: Record<string, () => unknown>,
  calls: string[],
) {
  const client = {
    subApp: 'note',
    invoke: async (channel: string) => {
      calls.push(channel);
      const a = answers[channel];
      if (!a) throw new Error(`unexpected ${channel}`);
      return a();
    },
    on: () => () => {},
  } as unknown as QrtzClient;
  const el = document.createElement('div');
  document.body.appendChild(el);
  const root = createRoot(el);
  return {
    el,
    root,
    render: () =>
      act(async () => {
        root.render(
          <ThemeProvider mode="light">
            <QrtzClientProvider client={client}>{ui}</QrtzClientProvider>
          </ThemeProvider>,
        );
        await new Promise((r) => setTimeout(r, 10));
      }),
  };
}
const q = (el: HTMLElement, id: string) =>
  el.querySelector<HTMLElement>(`[data-testid="${id}"]`);

describe("nano's calendar (ADR 0217)", () => {
  it('never asks macOS on mount; connect is the only ask', async () => {
    const calls: string[] = [];
    let s = status('not-determined', false);
    const m = mount(
      <Upcoming onRecord={() => {}} />,
      {
        'calendar:status': () => s,
        'calendar:upcoming': () => ({ events: [] }),
        'calendar:connect': () => (s = status('granted', true)),
      },
      calls,
    );
    await m.render();
    expect(calls).not.toContain('calendar:connect');
    expect(q(m.el, 'upcoming-connect')).not.toBeNull();
    await act(async () => {
      q(m.el, 'calendar-connect')!.click();
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(calls.filter((c) => c === 'calendar:connect')).toHaveLength(1);
    m.root.unmount();
  });

  it('lists upcoming events and records with the title', async () => {
    const calls: string[] = [];
    const titles: string[] = [];
    const m = mount(
      <Upcoming onRecord={(t) => titles.push(t)} />,
      {
        'calendar:status': () => status('granted', true),
        'calendar:upcoming': () => ({
          events: [
            {
              id: 'e1',
              calendarId: 'w',
              title: 'weekly sync',
              startAt: now + 600_000,
              endAt: now + 1_800_000,
              allDay: false,
              attendeeCount: 3,
            },
          ],
        }),
      },
      calls,
    );
    await m.render();
    expect(
      m.el.querySelectorAll('[data-testid="upcoming-event"]'),
    ).toHaveLength(1);
    expect(m.el.textContent).toContain('weekly sync');
    await act(async () => q(m.el, 'upcoming-record')!.click());
    expect(titles).toEqual(['weekly sync']);
    m.root.unmount();
  });

  it('settings: off writes enabled false, picking a calendar writes the list', async () => {
    const calls: string[] = [];
    const patches: unknown[] = [];
    const m = mount(
      <CalendarSettings patch={(p) => patches.push(p)} />,
      {
        'calendar:status': () => status('granted', true),
        'calendar:upcoming': () => ({ events: [] }),
      },
      calls,
    );
    await m.render();
    expect(q(m.el, 'note-settings-calendar')).not.toBeNull();
    await act(async () => q(m.el, 'calendar-pick-h')!.click());
    await act(async () => q(m.el, 'calendar-enabled')!.click());
    expect(patches).toEqual([
      { calendar: { calendars: ['w'] } },
      { calendar: { enabled: false } },
    ]);
    expect(calls).not.toContain('calendar:connect');
    m.root.unmount();
  });
});

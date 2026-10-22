// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { ThemeProvider } from '@qrtz/ui';
import type { SnapStatus } from '@qrtz/core';
import { SummonSection } from './SnapSettings';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let el: HTMLElement;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  el?.remove();
});
const q = (id: string) => el.querySelector(`[data-testid="${id}"]`);

const base: SnapStatus = {
  running: true,
  axTrusted: true,
  registered: {},
  refused: {},
};

function render(
  summon: { enabled?: boolean; bundleId?: string; name?: string },
  status: SnapStatus | null,
  onChange: (p: Record<string, unknown>) => void = () => {},
  onGrantInputMonitoring?: () => void,
) {
  el = document.createElement('div');
  document.body.appendChild(el);
  root = createRoot(el);
  act(() =>
    root!.render(
      <ThemeProvider mode="light">
        <SummonSection
          summon={summon}
          status={status}
          onChange={onChange}
          onGrantInputMonitoring={onGrantInputMonitoring}
        />
      </ThemeProvider>,
    ),
  );
}

describe('summon agent settings', () => {
  it('is off by default and shows no picker', () => {
    render({}, base);
    expect(q('snap-settings-summon')).not.toBeNull();
    expect(q('snap-summon-app')).toBeNull();
  });

  it('lists the helper’s apps and stores bundle id and name', () => {
    const seen: Record<string, unknown>[] = [];
    render(
      { enabled: true },
      {
        ...base,
        inputMonitoring: true,
        summonActive: true,
        apps: [
          { bundleId: 'com.openai.chat', name: 'ChatGPT' },
          { bundleId: 'com.apple.Safari', name: 'Safari' },
        ],
      },
      (p) => seen.push(p),
    );
    const sel = q('snap-summon-app') as HTMLSelectElement;
    expect(sel.options.length).toBe(3);
    act(() => {
      sel.value = 'com.openai.chat';
      sel.dispatchEvent(new Event('change', { bubbles: true }));
    });
    expect(seen).toEqual([{ bundleId: 'com.openai.chat', name: 'ChatGPT' }]);
    expect(q('snap-summon-ready')).not.toBeNull();
    expect(q('snap-summon-no-im')).toBeNull();
  });

  it('says when input monitoring is missing and when echo uses right ⌘', () => {
    render(
      { enabled: true, bundleId: 'com.x', name: 'X' },
      { ...base, inputMonitoring: false, summonClash: true, apps: [] },
    );
    expect(q('snap-summon-no-im')).not.toBeNull();
    expect(q('snap-summon-clash')?.textContent).toContain('summon is paused');
    // A chosen app that isn't in the list still shows by name.
    expect((q('snap-summon-app') as HTMLSelectElement).value).toBe('com.x');
  });

  it("input monitoring's prompt is the same grant button as the permissions page", () => {
    let asked = 0;
    render(
      { enabled: true },
      { ...base, inputMonitoring: false, apps: [] },
      () => {},
      () => asked++,
    );
    const prompt = q('snap-summon-no-im')!;
    expect(prompt.textContent).toContain('summon needs input monitoring');
    const btn = prompt.querySelector('button')!;
    expect(btn.textContent).toContain('input monitoring');
    act(() => btn.click());
    expect(asked).toBe(1);
    expect(el.textContent).toBe(el.textContent?.toLowerCase());
  });
});

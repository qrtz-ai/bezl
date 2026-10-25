// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { Onboarding, onboardingPermissions, setupState } from './Onboarding';
import { fakeHost } from './onboarding.fake';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const tick = () =>
  act(async () => {
    await new Promise((r) => setTimeout(r, 10));
  });

async function mount(host: ReturnType<typeof fakeHost>) {
  window.qrtzTransport = host.transport;
  const onDone = vi.fn();
  const el = document.createElement('div');
  document.body.appendChild(el);
  const root = createRoot(el);
  await act(async () => root.render(<Onboarding onDone={onDone} />));
  await tick();
  const q = (id: string) =>
    el.querySelector<HTMLElement>(`[data-testid="${id}"]`);
  const state = (id: string) => q(`onboarding-perm-${id}-state`)?.textContent;
  const click = async (id: string) => {
    await act(async () => q(id)!.click());
    await tick();
  };
  return { el, root, q, state, click, onDone };
}

afterEach(() => {
  delete (window as { qrtzTransport?: unknown }).qrtzTransport;
  document.body.innerHTML = '';
});

describe('setupState (ADR 0184)', () => {
  it('maps the live status to granted / not granted / needs restart / checking', () => {
    expect(setupState('granted', 'microphone', false)).toBe('granted');
    expect(setupState('denied', 'microphone', true)).toBe('not-granted');
    expect(setupState('not-determined', 'accessibility', false)).toBe(
      'not-granted',
    );
    // A screen recording grant reaches a running qrtz only after a relaunch.
    expect(setupState('denied', 'screen-recording', false)).toBe('not-granted');
    expect(setupState('denied', 'screen-recording', true)).toBe(
      'needs-restart',
    );
    expect(setupState('unsupported', 'microphone', false)).toBe('not-needed');
    expect(setupState(undefined, 'microphone', false)).toBe('checking');
    expect(setupState('unknown', 'microphone', false)).toBe('checking');
  });
});

describe('onboardingPermissions (ADR 0184 amendment)', () => {
  const ids = (cfg: unknown) => onboardingPermissions(cfg).map((d) => d.id);
  it('lists system audio only while nano is installed', () => {
    const nano = (installed: boolean) => ({ modules: { nano: { installed } } });
    expect(ids(nano(true))).toEqual([
      'microphone',
      'screen-recording',
      'accessibility',
    ]);
    expect(ids(nano(false))).toEqual(['microphone', 'accessibility']);
    // Before config is read: only what every install needs (no flash of a nano-only row).
    expect(ids(null)).toEqual(['microphone', 'accessibility']);
  });
  it('never throws on a config without that app entry (modules?.[only]?.installed)', () => {
    for (const cfg of [{}, { modules: {} }, { modules: { nano: null } }])
      expect(() => ids(cfg)).not.toThrow();
  });
});

describe('permissions onboarding (ADR 0184)', () => {
  it('without nano installed: only microphone and accessibility, never asks for system audio', async () => {
    const host = fakeHost({ nanoInstalled: false });
    const { el, state, q, click, root } = await mount(host);
    expect(state('microphone')).toBe('not granted');
    expect(state('accessibility')).toBe('not granted');
    expect(q('onboarding-perm-screen-recording-state')).toBeNull();
    expect(el.textContent).not.toContain('system audio');
    expect(el.textContent).not.toContain('record calls');
    expect(el.textContent).toBe(el.textContent!.toLowerCase());
    // Everything listed granted is enough to say open qrtz (system audio isn't needed yet).
    host.state.perms.microphone = 'granted';
    host.state.perms.accessibility = 'granted';
    await act(async () => window.dispatchEvent(new Event('focus')));
    await tick();
    expect(q('onboarding-continue')!.textContent).toBe('open qrtz');
    await click('onboarding-continue');
    expect(
      host
        .seen('permissions:request')
        .concat(host.seen('permissions:open-settings'))
        .map((c) => JSON.stringify(c.payload))
        .join(),
    ).not.toContain('screen-recording');
    act(() => root.unmount());
  });

  it('nano installed while it is open (config:changed): the system audio row appears', async () => {
    const host = fakeHost({ nanoInstalled: false });
    const { q, state, root } = await mount(host);
    expect(q('onboarding-perm-screen-recording-state')).toBeNull();
    host.state.nanoInstalled = true;
    await act(async () =>
      host.emit('config:changed', {
        onboarding: { done: false },
        modules: { nano: { installed: true } },
      }),
    );
    await tick();
    expect(state('screen-recording')).toBe('not granted');
    act(() => root.unmount());
  });

  it('shows microphone, system audio and accessibility with their live state, all lowercase', async () => {
    const host = fakeHost({ perms: { microphone: 'granted' } });
    const { el, state, q, root } = await mount(host);
    expect(state('microphone')).toBe('granted');
    expect(state('screen-recording')).toBe('not granted');
    expect(state('accessibility')).toBe('not granted');
    expect(q('onboarding-perm-microphone-allow')).toBeNull();
    expect(q('onboarding-perm-accessibility-allow')).not.toBeNull();
    expect(el.textContent).toContain('system audio');
    expect(el.textContent).toBe(el.textContent!.toLowerCase());
    // The shared page layout.
    expect(el.querySelector('[data-page]')).not.toBeNull();
    // Nothing downloads from onboarding.
    expect(host.state.calls.some((c) => c.channel.startsWith('speech:'))).toBe(
      false,
    );
    act(() => root.unmount());
  });

  it('allow asks macOS for that permission and the row follows the answer', async () => {
    const host = fakeHost();
    const { state, click, root } = await mount(host);
    await click('onboarding-perm-microphone-allow');
    expect(host.seen('permissions:request')[0]!.payload).toEqual({
      permission: 'microphone',
    });
    expect(state('microphone')).toBe('granted');
    act(() => root.unmount());
  });

  it('settings opens the pane; system audio not on yet says needs restart and offers restart qrtz', async () => {
    const host = fakeHost();
    host.state.grantOnRequest = false;
    const { state, click, q, root } = await mount(host);
    await click('onboarding-perm-screen-recording-open');
    expect(host.seen('permissions:open-settings')[0]!.payload).toEqual({
      pane: 'screen-recording',
    });
    expect(state('screen-recording')).toBe('needs restart');
    await click('onboarding-perm-screen-recording-restart');
    expect(host.seen('app:restart')).toHaveLength(1);
    // Accessibility asked but still off is just not granted (no restart needed).
    await click('onboarding-perm-accessibility-allow');
    expect(state('accessibility')).toBe('not granted');
    expect(q('onboarding-perm-accessibility-restart')).toBeNull();
    act(() => root.unmount());
  });

  it('re-reads the status when the window regains focus, and on permissions:changed', async () => {
    const host = fakeHost();
    const { state, root } = await mount(host);
    const before = host.seen('permissions:get').length;
    host.state.perms.accessibility = 'granted';
    await act(async () => window.dispatchEvent(new Event('focus')));
    await tick();
    expect(host.seen('permissions:get').length).toBe(before + 1);
    expect(state('accessibility')).toBe('granted');
    await act(async () =>
      host.emit('permissions:changed', {
        ...host.state.perms,
        microphone: 'granted',
      }),
    );
    await tick();
    expect(state('microphone')).toBe('granted');
    act(() => root.unmount());
  });

  it.each(['onboarding-continue', 'onboarding-skip'])(
    '%s finishes through the host (onboarding:finish), never config:set',
    async (button) => {
      const host = fakeHost();
      const { click, onDone, root } = await mount(host);
      await click(button);
      expect(host.seen('onboarding:finish')).toHaveLength(1);
      expect(host.seen('config:set')).toHaveLength(0);
      expect(host.state.done).toBe(true);
      expect(onDone).toHaveBeenCalledOnce();
      act(() => root.unmount());
    },
  );

  it('keeps the continue button hidden until config:get answers', async () => {
    const host = fakeHost({
      perms: {
        microphone: 'granted',
        'screen-recording': 'granted',
        accessibility: 'granted',
      },
    });
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const invoke = host.transport.invoke;
    host.transport.invoke = async (e) => {
      if (e.channel === 'config:get') await gate;
      return invoke(e);
    };
    const { q, root } = await mount(host);
    expect(q('onboarding-continue')).toBeNull();
    expect(q('onboarding-skip')).not.toBeNull();
    await act(async () => release());
    await tick();
    expect(q('onboarding-continue')!.textContent).toBe('open qrtz');
    act(() => root.unmount());
  });

  it('says open qrtz when everything is granted', async () => {
    const host = fakeHost({
      perms: {
        microphone: 'granted',
        'screen-recording': 'granted',
        accessibility: 'granted',
      },
    });
    const { q, root } = await mount(host);
    expect(q('onboarding-continue')!.textContent).toBe('open qrtz');
    act(() => root.unmount());
  });
});

// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { AppList, AppRow } from './Marketplace';
import { PermissionSetupList, PermissionSetupRow } from './Onboarding';

// WCAG 2.5.3 (axe label-content-name-mismatch, which needs a real layout: e2e-tauri/onboarding.spec.ts runs it in
// Chromium): a button's accessible name must contain the text it shows.
const noop = () => {};
const words = (s: string) => s.replace(/\s+/g, ' ').trim();

describe('AppRow details button', () => {
  const html = renderToStaticMarkup(
    <AppList aria-label="installed">
      <AppRow
        name="echo"
        icon="mic"
        description="Dictation anywhere"
        meta="v0.2.0 · built in"
        onOpen={noop}
      />
    </AppList>,
  );

  it("has no aria-label: it is named by the row's visible text, then a hidden 'details'", () => {
    document.body.innerHTML = html;
    const b = document.querySelector<HTMLElement>(
      '[data-testid="app-open-echo"]',
    )!;
    expect(b.tagName).toBe('BUTTON');
    expect(b.hasAttribute('aria-label')).toBe(false);
    expect(b.hasAttribute('aria-labelledby')).toBe(false);
    // The name (its text content) contains everything the row shows, and ends in details.
    expect(words(b.textContent!)).toMatch(/details$/);
    for (const visible of ['echo', 'v0.2.0 · built in', 'dictation anywhere'])
      expect(b.textContent).toContain(visible);
    const hidden = [...b.querySelectorAll<HTMLElement>('span')].find(
      (s) => s.textContent === ' details',
    )!;
    expect(hidden.style.position).toBe('absolute');
    expect(hidden.style.overflow).toBe('hidden');
  });
});

describe('PermissionSetupRow buttons', () => {
  it("each aria-label contains the button's visible text and names the permission", () => {
    document.body.innerHTML = renderToStaticMarkup(
      <PermissionSetupList aria-label="permissions">
        <PermissionSetupRow
          id="microphone"
          icon="mic"
          label="microphone"
          reason="echo hears you"
          state="not-granted"
          onAllow={noop}
          onOpenSettings={noop}
        />
        <PermissionSetupRow
          id="screen-recording"
          icon="screen-recording"
          label="system audio"
          reason="nano records the call"
          state="needs-restart"
          onOpenSettings={noop}
          onRestart={noop}
        />
      </PermissionSetupList>,
    );
    const buttons = [...document.querySelectorAll('button')];
    expect(buttons.map((b) => b.getAttribute('aria-label'))).toEqual([
      'allow microphone',
      'microphone settings',
      'system audio settings',
      null,
    ]);
    for (const b of buttons) {
      const label = b.getAttribute('aria-label');
      if (label) expect(label).toContain(words(b.textContent!));
    }
    expect(
      document.querySelector(
        '[data-testid="onboarding-perm-screen-recording-state"]',
      )!.textContent,
    ).toBe('needs restart');
  });
});

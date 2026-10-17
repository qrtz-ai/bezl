// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { defineSubApp } from '@qrtz/core';
import { SubAppHost } from './SubAppHost';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

describe('SubAppHost error boundary (M0.9)', () => {
  it('shows a fallback for a crashing sub app without taking down the shell', async () => {
    const boom = defineSubApp({
      manifest: { id: 'boom', name: 'Boom', icon: '!' },
      load: async () => ({
        default: () => {
          throw new Error('kaboom');
        },
      }),
    });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const root = createRoot(el);
    const origError = console.error;
    console.error = () => {};
    try {
      await act(async () => {
        root.render(
          <div>
            <nav data-testid="shell-nav">shell</nav>
            <SubAppHost app={boom} />
          </div>,
        );
      });
      await act(async () => {
        await new Promise((r) => setTimeout(r, 20));
      });
    } finally {
      console.error = origError;
    }
    expect(
      el.querySelector('[data-testid="subapp-error"]')?.textContent,
    ).toContain('kaboom');
    expect(el.querySelector('[data-testid="shell-nav"]')).not.toBeNull();
    root.unmount();
  });
});

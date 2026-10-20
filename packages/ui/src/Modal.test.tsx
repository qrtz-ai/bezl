// @vitest-environment jsdom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import { Modal } from './Modal';
import { ModuleSettingsModal } from './ModuleSettings';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
let cleanup: (() => void) | undefined;
afterEach(() => cleanup?.());

function mount(node: React.ReactNode) {
  const el = document.createElement('div');
  document.body.append(el);
  const root = createRoot(el);
  act(() => root.render(node));
  cleanup = () => {
    act(() => root.unmount());
    el.remove();
  };
}
const noop = () => {};

describe('modal initial focus', () => {
  it('focuses the control marked data-autofocus', () => {
    mount(
      <Modal open onClose={noop} title="t">
        <input data-testid="a" />
        <input data-testid="b" data-autofocus />
      </Modal>,
    );
    expect(document.activeElement?.getAttribute('data-testid')).toBe('b');
  });
  it('focuses the first control by default', () => {
    mount(
      <Modal open onClose={noop} title="t">
        <input data-testid="a" />
      </Modal>,
    );
    expect(document.activeElement?.tagName).toBe('BUTTON');
  });
  it('the opts modal focuses the dialog itself, not its close button', () => {
    mount(
      <ModuleSettingsModal module="echo" open onClose={noop} data-testid="m">
        <input />
      </ModuleSettingsModal>,
    );
    expect(document.activeElement?.getAttribute('role')).toBe('dialog');
  });
  it('the opts modal still honours data-autofocus', () => {
    mount(
      <ModuleSettingsModal module="echo" open onClose={noop}>
        <input data-testid="x" data-autofocus />
      </ModuleSettingsModal>,
    );
    expect(document.activeElement?.getAttribute('data-testid')).toBe('x');
  });
});

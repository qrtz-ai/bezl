// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { ReactNode } from 'react';
import { ThemeProvider } from './theme';
import { Callout } from './Callout';
import { Banner } from './Meeting';
import { AskCard, ConfirmCard, StopBar } from './Ctrl';
import { CaptureNotice } from './Lifecycle';
import { PermissionPrompt } from './Permission';
import { ModelRequired } from './ModelRequirement';

const noop = () => {};

/** Every notice qrtz shows is a Callout (QRT-302): same outline, same dot, same slots. */
const NOTICES: [string, ReactNode][] = [
  ['callout', <Callout title="saved" />],
  ['banner', <Banner>standup</Banner>],
  [
    'ask card',
    <AskCard
      question="which doc"
      options={[{ id: 'a', label: 'a' }]}
      onPick={noop}
      onCancel={noop}
    />,
  ],
  [
    'confirm card',
    <ConfirmCard
      action="click send"
      app="mail"
      rules={['label:send']}
      onApprove={noop}
      onDeny={noop}
    />,
  ],
  ['stop bar', <StopBar onStop={noop} />],
  [
    'permission prompt',
    <PermissionPrompt
      title="turn on microphone"
      pane="microphone"
      onOpen={noop}
    />,
  ],
  [
    'capture notice',
    <CaptureNotice items={[{ id: 'mic', text: 'mic is quiet' }]} />,
  ],
];

function mount(node: ReactNode) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const root = createRoot(el);
  act(() => root.render(<ThemeProvider mode="dark">{node}</ThemeProvider>));
  return { el, unmount: () => (root.unmount(), el.remove()) };
}

describe('one alert component (QRT-302)', () => {
  it.each(NOTICES)('%s renders a Callout root', (_, node) => {
    const v = mount(node);
    expect(v.el.firstElementChild?.hasAttribute('data-callout')).toBe(true);
    v.unmount();
  });

  it('the required-model banner is the one page-level callout', () => {
    const v = mount(
      <ModelRequired
        app="echo"
        name="speech model"
        state="missing"
        sizeBytes={1}
        onDownload={noop}
      >
        <p>body</p>
      </ModelRequired>,
    );
    expect(
      v.el.querySelectorAll('[data-callout][data-level="page"]'),
    ).toHaveLength(1);
    v.unmount();
  });

  it('keeps technical detail collapsed', () => {
    const v = mount(
      <Callout
        tone="danger"
        title="couldn't save"
        details="EACCES /tmp/x"
        data-testid="c"
      />,
    );
    const d = v.el.querySelector<HTMLDetailsElement>(
      '[data-testid="c-details"]',
    );
    expect(d?.open).toBe(false);
    expect(d?.textContent).toContain('EACCES');
    v.unmount();
  });
});

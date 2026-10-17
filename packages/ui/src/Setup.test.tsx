import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import {
  SpeechNotice,
  SpeechSetupPanel,
  formatBytes,
  type SpeechSetupView,
} from './Setup';

const view = (
  download: SpeechSetupView['download'],
  patch: Partial<SpeechSetupView> = {},
): SpeechSetupView => ({
  configured: true,
  installed: null,
  bytes: 0,
  latest: '1.0.0',
  latestBytes: 412_000_000,
  waiting: null,
  listExpired: false,
  download,
  ...patch,
});
const noop = () => {};
const panel = (v: SpeechSetupView | null, manage = false) =>
  renderToStaticMarkup(
    <SpeechSetupPanel
      status={v}
      manage={manage}
      onDownload={noop}
      onPause={noop}
      onResume={noop}
      onDelete={noop}
    />,
  );

describe('speech setup', () => {
  it('formats sizes lowercase', () => {
    expect(formatBytes(0)).toBe('0 mb');
    expect(formatBytes(412_000_000)).toBe('412 mb');
    expect(formatBytes(1_600_000_000)).toBe('1.6 gb');
    expect(formatBytes(12_000)).toBe('12 kb');
  });

  it('not downloaded: size and a download button', () => {
    const h = panel(view({ state: 'idle', done: 0, total: 0 }));
    expect(h).toContain('speech · 412 mb');
    expect(h).toContain('data-testid="speech-download"');
  });

  it('downloading: progress and pause; paused: resume', () => {
    const d = panel(
      view({ state: 'downloading', done: 206_000_000, total: 412_000_000 }),
    );
    expect(d).toContain('aria-valuenow="50"');
    expect(d).toContain('206 mb of 412 mb');
    expect(d).toContain('data-testid="speech-pause"');
    const p = panel(view({ state: 'paused', done: 1, total: 2 }));
    expect(p).toContain('data-testid="speech-resume"');
    expect(p).not.toContain('data-testid="speech-pause"');
  });

  it('error: the message with retry', () => {
    const h = panel(
      view({
        state: 'error',
        done: 0,
        total: 0,
        error: "couldn't reach the download server",
      }),
    );
    expect(h).toContain('data-testid="speech-retry"');
    expect(h).toContain('couldn&#x27;t reach the download server');
  });

  it('installed: version and size; settings adds update, re-download and delete', () => {
    const v = view(
      { state: 'idle', done: 0, total: 0 },
      {
        installed: '1.0.0',
        bytes: 412_000_000,
        latest: '1.1.0',
      },
    );
    expect(panel(v)).toContain('speech 1.0.0 · 412 mb');
    expect(panel(v)).not.toContain('speech-delete');
    const m = panel(v, true);
    expect(m).toContain('update to 1.1.0');
    expect(m).toContain('data-testid="speech-redownload"');
    expect(m).toContain('data-testid="speech-delete"');
  });

  it('a build without a speech module says so; nothing in note/talk', () => {
    const v = view({ state: 'idle', done: 0, total: 0 }, { configured: false });
    expect(panel(v)).toContain('speech isn&#x27;t available in this build');
    expect(
      renderToStaticMarkup(<SpeechNotice status={v} onOpenSettings={noop} />),
    ).toBe('');
  });

  it('note/talk notice: hidden once installed', () => {
    const v = view(
      { state: 'idle', done: 0, total: 0 },
      { installed: '1.0.0' },
    );
    expect(
      renderToStaticMarkup(<SpeechNotice status={v} onOpenSettings={noop} />),
    ).toBe('');
    const missing = renderToStaticMarkup(
      <SpeechNotice
        status={view({ state: 'idle', done: 0, total: 0 })}
        onOpenSettings={noop}
      />,
    );
    expect(missing).toContain('speech isn&#x27;t downloaded');
  });
});

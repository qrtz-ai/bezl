import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { CaptureNotice, CaptureStatus } from './Lifecycle';

describe('CaptureStatus silent sources (N29)', () => {
  it('one badge per silent source, lowercase', () => {
    const html = renderToStaticMarkup(
      <CaptureStatus
        degraded={[]}
        silent={[
          { source: 'mic', since: '10:04' },
          { source: 'system', since: '10:06' },
        ]}
      />,
    );
    expect(html).toContain('data-testid="capture-silent-mic"');
    expect(html).toContain('no audio from mic since 10:04');
    expect(html).toContain('no audio from system audio since 10:06');
    expect(html).not.toContain('capture-no-audio');
  });

  it('noAudio replaces the per-source badges with one stronger badge', () => {
    const html = renderToStaticMarkup(
      <CaptureStatus
        degraded={[]}
        silent={[{ source: 'mic', since: '10:04' }]}
        noAudio
      />,
    );
    expect(html).toContain('data-testid="capture-no-audio"');
    expect(html).toContain('role="alert"');
    expect(html).toContain('no audio is being recorded');
    expect(html).not.toContain('capture-silent-mic');
  });

  it('nothing to show renders nothing', () => {
    expect(renderToStaticMarkup(<CaptureStatus degraded={[]} />)).toBe('');
    expect(renderToStaticMarkup(<CaptureNotice items={[]} />)).toBe('');
  });

  it('CaptureNotice: one line per item', () => {
    const html = renderToStaticMarkup(
      <CaptureNotice
        items={[
          { id: 'silent-0', text: 'no audio from mic 10:04–10:09' },
          {
            id: 'fallback-0',
            text: 'screen capture failed, no audio source left',
            detail: 'denied',
          },
        ]}
      />,
    );
    expect(html).toContain('data-testid="capture-notice-silent-0"');
    expect(html).toContain('data-testid="capture-notice-fallback-0"');
    expect(html).toContain('title="denied"');
  });
});

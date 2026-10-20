import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { TalkPill } from './Talk';

const REASON =
  "the capture helper isn't running, so the mic can't open (echo: signature mismatch: the helper isn't signed with qrtz's Developer ID identity)";

describe('TalkPill error (QRT-291)', () => {
  it('shows the whole reason and wraps instead of truncating', () => {
    const html = renderToStaticMarkup(
      <TalkPill inline phase="error" level={0} error={REASON} />,
    );
    expect(html).toContain('signature mismatch');
    expect(html).toContain('developer id identity)');
    expect(html).not.toContain('text-overflow:ellipsis');
    expect(html).not.toContain('white-space:nowrap');
    expect(html).toContain('white-space:normal');
    expect(html).toContain('height:auto');
  });

  it('keeps the fixed pill size when not failed', () => {
    const html = renderToStaticMarkup(
      <TalkPill inline phase="listening" level={0.5} />,
    );
    expect(html).toContain('height:44px');
  });
});

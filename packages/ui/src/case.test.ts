import { describe, expect, it } from 'vitest';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { lowerTitle } from './case';
import { Heading } from './Heading';
import { SettingsSection } from './Layout';

describe('lowercase titles (brand rule)', () => {
  it('lowerTitle lowercases strings only', () => {
    expect(lowerTitle('Settings')).toBe('settings');
    expect(lowerTitle(['A', 1])).toEqual(['a', 1]);
    const el = createElement('b', null, 'X');
    expect(lowerTitle(el)).toBe(el);
  });
  it('Heading and SettingsSection render lowercase; preserveCase keeps user content', () => {
    expect(
      renderToStaticMarkup(createElement(Heading, { children: 'Settings' })),
    ).toContain('>settings<');
    expect(
      renderToStaticMarkup(
        createElement(Heading, { preserveCase: true, children: 'Weekly Sync' }),
      ),
    ).toContain('Weekly Sync');
    const s = renderToStaticMarkup(
      createElement(SettingsSection, { title: 'Privacy', children: null }),
    );
    expect(s).toContain('privacy');
    expect(s).not.toContain('uppercase');
  });
});

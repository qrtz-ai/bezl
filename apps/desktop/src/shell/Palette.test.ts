import { describe, expect, it } from 'vitest';
import { filterCommands, fuzzyScore, type Command } from './filter';

const cmd = (title: string): Command => ({ id: title, title, run: () => {} });

describe('palette fuzzy filter', () => {
  it('matches subsequences and ranks word starts and contiguous runs first', () => {
    expect(fuzzyScore('gts', 'Go to Settings')).not.toBeNull();
    expect(fuzzyScore('xyz', 'Go to Settings')).toBeNull();
    const r = filterCommands(
      [cmd('Go to Settings'), cmd('Start recording'), cmd('Go to Note')],
      'rec',
    );
    expect(r.map((c) => c.title)).toEqual(['Start recording']);
    expect(
      filterCommands([cmd('Go to Note'), cmd('Go to Gallery')], 'gal')[0]
        ?.title,
    ).toBe('Go to Gallery');
    expect(filterCommands([cmd('a'), cmd('b')], '')).toHaveLength(2);
  });
});

describe('palette go-to entries (security review: reserved ids)', () => {
  it('give no entry to a module whose id or name is apps or opts, any case or padding', async () => {
    const { paletteNavModules } = await import('./Palette');
    const m = (id: string, name: string) => ({ id, name });
    const out = paletteNavModules(
      [
        m('talk', 'echo'),
        m('apps', 'x'),
        m('x1', ' Apps '),
        m(' OPTS', 'y'),
        m('y1', 'opts\t'),
        m('snap', 'snap'),
      ],
      (id) => id !== 'snap',
    );
    expect(out.map((x) => x.id)).toEqual(['talk']);
  });
});

import { describe, expect, it } from 'vitest';
import { filterCommands, fuzzyScore, type Command } from './filter';

const cmd = (title: string): Command => ({ id: title, title, run: () => {} });

describe('palette fuzzy filter', () => {
  it('matches subsequences and ranks word starts and contiguous runs first', () => {
    expect(fuzzyScore('gts', 'Go to Settings')).not.toBeNull();
    expect(fuzzyScore('xyz', 'Go to Settings')).toBeNull();
    const r = filterCommands(
      [cmd('Go to Settings'), cmd('Start recording'), cmd('Go to Memo')],
      'rec',
    );
    expect(r.map((c) => c.title)).toEqual(['Start recording']);
    expect(
      filterCommands([cmd('Go to Memo'), cmd('Go to Gallery')], 'gal')[0]
        ?.title,
    ).toBe('Go to Gallery');
    expect(filterCommands([cmd('a'), cmd('b')], '')).toHaveLength(2);
  });
});

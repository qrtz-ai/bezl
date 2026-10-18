import { describe, expect, it } from 'vitest';
import {
  meetingPath,
  onNavigateRequest,
  parseMeetingPath,
  requestNavigate,
} from './nav';

describe('nav requests', () => {
  it('notifies listeners with the whole request until they unsubscribe', () => {
    const got: unknown[] = [];
    const off = onNavigateRequest((r) => got.push(r));
    requestNavigate({ app: 'note', meetingId: 'm1', tab: 'summary' });
    expect(got).toEqual([{ app: 'note', meetingId: 'm1', tab: 'summary' }]);
    off();
    requestNavigate({ app: 'talk' });
    expect(got).toHaveLength(1);
  });
});

describe('meeting paths', () => {
  it('round-trip, with the default tab left out', () => {
    expect(meetingPath('m1')).toBe('meetings/m1');
    expect(meetingPath('m1', 'transcript')).toBe('meetings/m1');
    expect(meetingPath('m1', 'summary')).toBe('meetings/m1/summary');
    expect(meetingPath('a b/c')).toBe('meetings/a%20b%2Fc');
    for (const tab of ['transcript', 'summary', 'notes'] as const)
      expect(parseMeetingPath(meetingPath('a b/c', tab))).toEqual({
        meetingId: 'a b/c',
        tab,
      });
  });

  it('the list, other paths and unknown tabs parse to no meeting / the default tab', () => {
    const none = { meetingId: null, tab: 'transcript' };
    expect(parseMeetingPath('')).toEqual(none);
    expect(parseMeetingPath('meetings')).toEqual(none);
    expect(parseMeetingPath('other/m1')).toEqual(none);
    expect(parseMeetingPath('meetings/m1/bogus')).toEqual({
      meetingId: 'm1',
      tab: 'transcript',
    });
  });
});

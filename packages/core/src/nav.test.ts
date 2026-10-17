import { describe, expect, it } from 'vitest';
import { onNavigateRequest, requestNavigate, takePendingMeeting } from './nav';

describe('nav requests', () => {
  it('notifies listeners and hands the pending meeting to exactly one taker', () => {
    const got: string[] = [];
    const off = onNavigateRequest((r) => got.push(r.app));
    requestNavigate({ app: 'memo', meetingId: 'm1' });
    expect(got).toEqual(['memo']);
    expect(takePendingMeeting()).toBe('m1');
    expect(takePendingMeeting()).toBeUndefined();
    off();
    requestNavigate({ app: 'home' });
    expect(got).toEqual(['memo']);
  });
});

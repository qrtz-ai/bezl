import { describe, expect, it } from 'vitest';
import { formatBytes } from './ModelRequirement';

describe('model sizes', () => {
  it('formats sizes lowercase', () => {
    expect(formatBytes(0)).toBe('0 mb');
    expect(formatBytes(412_000_000)).toBe('412 mb');
    expect(formatBytes(1_600_000_000)).toBe('1.6 gb');
    expect(formatBytes(12_000)).toBe('12 kb');
  });
});

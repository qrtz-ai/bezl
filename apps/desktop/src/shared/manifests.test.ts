import { describe, expect, it } from 'vitest';
import { manifests } from './manifests';

describe('manifests', () => {
  it('are unique and least-privilege for home', () => {
    const ids = manifests.map((m) => m.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(manifests.find((m) => m.id === 'home')?.capabilities).toEqual([
      'app.info',
    ]);
  });
});

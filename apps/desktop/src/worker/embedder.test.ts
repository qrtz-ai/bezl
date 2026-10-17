import { describe, expect, it } from 'vitest';
import { chooseBackend } from './embedder';

describe('chooseBackend', () => {
  it('uses native when its binary loads', () => {
    expect(chooseBackend(undefined, () => true)).toBe('native');
  });
  it('falls back to WASM when native is unavailable (Intel Macs)', () => {
    expect(chooseBackend(undefined, () => false)).toBe('wasm');
  });
  it('honours QRTZ_EMBEDDINGS_BACKEND', () => {
    expect(chooseBackend('wasm', () => true)).toBe('wasm');
    expect(chooseBackend('native', () => false)).toBe('native');
  });
});

import { describe, expect, it } from 'vitest';
import * as C from '../src/constants';
import {
  CODE_ALPHABET,
  CODE_SECRET_BITS,
  CODE_SECRET_CHARS,
  newCode,
} from '../src/crypto';

// QRT-316 (Bozniak): pin the enrollment secret's size. With ENROLL_CODE_STRIKES tries per code, an online guess
// succeeds with probability strikes / 30^17 (about 5 / 2^83.4). Changing either number must fail here first.
describe('enrollment secret entropy (QRT-316)', () => {
  it('secret is 17 characters from a 30-symbol alphabet, about 83.4 bits', () => {
    expect(CODE_SECRET_CHARS).toBe(17);
    expect(CODE_ALPHABET.length).toBe(30);
    expect(new Set(CODE_ALPHABET).size).toBe(30);
    expect(CODE_SECRET_BITS).toBeCloseTo(17 * Math.log2(30), 10);
    expect(CODE_SECRET_BITS).toBeGreaterThan(83.4);
    expect(CODE_SECRET_BITS).toBeLessThan(83.5);
  });

  it('issued codes carry exactly that secret', () => {
    for (let i = 0; i < 50; i++) {
      const { secret } = newCode();
      expect(secret).toHaveLength(17);
      for (const ch of secret) expect(CODE_ALPHABET).toContain(ch);
    }
  });

  it('guess odds per code stay below 1e-24', () => {
    expect(C.ENROLL_CODE_STRIKES).toBe(5);
    expect(C.ENROLL_CODE_STRIKES / 2 ** CODE_SECRET_BITS).toBeLessThan(1e-24);
  });
});

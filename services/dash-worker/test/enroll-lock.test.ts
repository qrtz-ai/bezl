// Bozniak hardening item 4: enrollment guessing. Failures are counted per client source (and per dashboard, which
// rotates its codes, see api.test.ts); a lock is not lifted by a later success; code lookup is by HMAC (no
// plaintext compare, so no timing on the code); an unknown and a wrong code get the same answer.
import { env } from 'cloudflare:workers';
import { describe, expect, it } from 'vitest';
import * as C from '../src/constants';

describe('enroll lock (item 4)', () => {
  it('a locked source stays locked even when it then presents a valid code', async () => {
    const dir = env.ENROLL.getByName('enroll');
    const src = { narrow: 'lock-test-narrow', wide: null };
    const now = Date.now();
    for (let i = 0; i < C.ENROLL_IP_FAILS_BEFORE_LOCK; i++)
      await dir.fail(src, now - 120_000 + i);
    await dir.register('valid-code-hash', 'dash_x', now + 60_000);
    const r = await dir.claim(src, 'valid-code-hash', now);
    expect(r.ok).toBe(false);
    expect(!r.ok && r.status).toBe(429);
    // The code was not consumed by the locked attempt, and another source can still use it.
    const other = await dir.claim(
      { narrow: 'lock-test-other', wide: null },
      'valid-code-hash',
      now,
    );
    expect(other.ok).toBe(true);
    // A success elsewhere did not lift the lock.
    const again = await dir.claim(src, 'nope', now + 1000);
    expect(!again.ok && again.status).toBe(429);
  });

  it('unknown and expired codes get the same answer', async () => {
    const dir = env.ENROLL.getByName('enroll');
    const now = Date.now();
    await dir.register('expired-hash', 'dash_y', now - 1);
    const a = await dir.claim(
      { narrow: 'same-a', wide: null },
      'never-issued',
      now,
    );
    const b = await dir.claim(
      { narrow: 'same-b', wide: null },
      'expired-hash',
      now,
    );
    expect(a).toEqual(b);
  });
});

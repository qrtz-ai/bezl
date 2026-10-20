// QRT-284 (Bozniak): enrollment codes are `<id>.<secret>`, only an HMAC of the secret is stored, 5 wrong secrets on
// an id kill that code, and unknown / wrong / dead codes all get the same 401.
import { env, exports } from 'cloudflare:workers';
import { runInDurableObject } from 'cloudflare:test';
import { describe, expect, it } from 'vitest';
import * as C from '../src/constants';
import {
  CODE_ALPHABET,
  CODE_SECRET_BITS,
  CODE_SECRET_CHARS,
  newCode,
  parseCode,
} from '../src/crypto';
import type { Dashboard } from '../src/dashboard';
import type { EnrollDirectory } from '../src/directory';

let ipN = 0;
const ip = () => `198.18.${Math.floor(++ipN / 250)}.${ipN % 250}`;
const post = async (
  path: string,
  body?: unknown,
  bearer?: string,
  from = ip(),
) => {
  const headers: Record<string, string> = {
    'content-type': 'application/json',
    'cf-connecting-ip': from,
  };
  if (bearer) headers.authorization = `Bearer ${bearer}`;
  const r = await exports.default.fetch(
    new Request(`https://dash.qrtz.ai${path}`, {
      method: 'POST',
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    }),
  );
  return { status: r.status, text: await r.text(), headers: r.headers };
};
const setup = async () => {
  await runInDurableObject(
    env.ENROLL.getByName('enroll'),
    (_i: EnrollDirectory, s) => {
      s.storage.sql.exec("DELETE FROM meta WHERE k = 'setup_used'");
    },
  );
  const r = await post('/v1/owner/setup', undefined, env.DASH_SETUP_TOKEN);
  expect(r.status).toBe(201);
  const j = JSON.parse(r.text);
  return { dashId: j.dashboard_id as string, owner: j.owner_key as string };
};
const issue = async (owner: string) => {
  const r = await post('/v1/owner/codes', undefined, owner);
  expect(r.status).toBe(201);
  return JSON.parse(r.text).code as string;
};
const enroll = (code: string, name = 'cap bot') =>
  post('/v1/enroll', { code, name });
/** Same id, a different (wrong) secret. */
const wrongSecret = (code: string) => {
  const [id, secret] = code.split('.') as [string, string];
  const flip = secret[0] === '2' ? '3' : '2';
  return `${id}.${flip}${secret.slice(1)}`;
};

describe('code format and entropy (QRT-284)', () => {
  it('codes are <prefix><id>.<secret> with >= 80 bits of secret', async () => {
    expect(CODE_SECRET_BITS).toBeGreaterThanOrEqual(80);
    const { owner } = await setup();
    const code = await issue(owner);
    expect(code).toMatch(
      new RegExp(
        `^ENR-[${CODE_ALPHABET}]{8}\\.[${CODE_ALPHABET}]{${CODE_SECRET_CHARS}}$`,
      ),
    );
    expect(parseCode(` ${code.toLowerCase()} `)).toEqual({
      id: code.split('.')[0],
      secret: code.split('.')[1],
    });
    expect(parseCode('ENR-AAAA-BBBB')).toBeNull();
  });

  it('secrets do not repeat and use the whole alphabet', () => {
    const seen = new Set<string>();
    const chars = new Set<string>();
    for (let i = 0; i < 2000; i++) {
      const { secret } = newCode();
      seen.add(secret);
      for (const c of secret) chars.add(c);
    }
    expect(seen.size).toBe(2000);
    expect(chars.size).toBe(CODE_ALPHABET.length);
  });
});

describe('storage (QRT-284)', () => {
  it('no plaintext secret or code is stored, only an HMAC', async () => {
    const { dashId, owner } = await setup();
    const code = await issue(owner);
    const secret = code.split('.')[1]!;
    const dash = await runInDurableObject(
      env.DASHBOARD.getByName(dashId),
      (i: Dashboard) => i.dumpForTest(),
    );
    const dir = await runInDurableObject(
      env.ENROLL.getByName('enroll'),
      (_i: EnrollDirectory, s) =>
        s.storage.sql.exec('SELECT * FROM enroll_codes').toArray(),
    );
    for (const blob of [JSON.stringify(dash), JSON.stringify(dir)]) {
      expect(blob).not.toContain(secret);
      expect(blob).not.toContain(code);
    }
    const row = (dash.codes as { secret_mac: string }[])[0]!;
    expect(row.secret_mac).toMatch(/^[0-9a-f]{64}$/);
  });
});

describe('per-code strikes (QRT-284)', () => {
  it('5 wrong secrets on an id kill the code, even for the right secret afterwards', async () => {
    const { owner } = await setup();
    const code = await issue(owner);
    for (let i = 0; i < C.ENROLL_CODE_STRIKES; i++)
      expect((await enroll(wrongSecret(code))).status).toBe(401);
    expect((await enroll(code)).status).toBe(401);
  });

  it('4 wrong secrets leave the code usable', async () => {
    const { owner } = await setup();
    const code = await issue(owner);
    for (let i = 0; i < C.ENROLL_CODE_STRIKES - 1; i++)
      expect((await enroll(wrongSecret(code))).status).toBe(401);
    expect((await enroll(code, 'four strike bot')).status).toBe(201);
  });

  it('strikes on one code do not touch another', async () => {
    const { owner } = await setup();
    const a = await issue(owner);
    const b = await issue(owner);
    for (let i = 0; i < C.ENROLL_CODE_STRIKES; i++)
      await enroll(wrongSecret(a));
    expect((await enroll(b, 'other code bot')).status).toBe(201);
  });

  it('unknown id, wrong secret, dead, used, expired and malformed codes get the same 401', async () => {
    const { dashId, owner } = await setup();
    const dead = await issue(owner);
    for (let i = 0; i < C.ENROLL_CODE_STRIKES; i++)
      await enroll(wrongSecret(dead));
    const live = await issue(owner);
    const used = await issue(owner);
    expect((await enroll(used, 'used code bot')).status).toBe(201);
    const expired = await issue(owner);
    const expiredId = expired.split('.')[0]!;
    await runInDurableObject(
      env.DASHBOARD.getByName(dashId),
      (_i: Dashboard, s) => {
        s.storage.sql.exec(
          'UPDATE enroll_code_macs SET expires_at = ? WHERE code_id = ?',
          Date.now() - 1,
          expiredId,
        );
      },
    );
    await runInDurableObject(
      env.ENROLL.getByName('enroll'),
      (_i: EnrollDirectory, s) => {
        s.storage.sql.exec(
          'UPDATE enroll_codes SET expires_at = ? WHERE code_id = ?',
          Date.now() - 1,
          expiredId,
        );
      },
    );
    const unknown = `ENR-${'2'.repeat(8)}.${'3'.repeat(CODE_SECRET_CHARS)}`;
    const answers = await Promise.all(
      [
        unknown,
        wrongSecret(live),
        dead,
        used,
        expired,
        'ENR-AAAA-BBBB',
        '',
      ].map((c) => enroll(c)),
    );
    for (const r of answers) {
      expect(r.status).toBe(401);
      expect(r.text).toBe(answers[0]!.text);
      expect([...r.headers].sort()).toEqual([...answers[0]!.headers].sort());
    }
    // The live code still works (one strike only).
    expect((await enroll(live, 'live code bot')).status).toBe(201);
  });

  it('a locked ip still gets 429, before any code is looked at', async () => {
    const { owner } = await setup();
    const code = await issue(owner);
    const from = '198.19.0.1';
    let last = 0;
    for (let i = 0; i < C.ENROLL_IP_FAILS_PER_MIN + 1; i++)
      last = (
        await post(
          '/v1/enroll',
          { code: `ENR-${'2'.repeat(8)}.${'4'.repeat(17)}`, name: 'x' },
          undefined,
          from,
        )
      ).status;
    expect(last).toBe(429);
    expect(
      (await post('/v1/enroll', { code, name: 'x bot' }, undefined, from))
        .status,
    ).toBe(429);
    // Not consumed and not struck: it works from elsewhere.
    expect((await enroll(code, 'after lock bot')).status).toBe(201);
  });

  it('TTL is 15 minutes and codes are single use', async () => {
    expect(C.ENROLL_TTL_MS).toBe(15 * 60 * 1000);
    const { owner } = await setup();
    const r = await post('/v1/owner/codes', undefined, owner);
    const j = JSON.parse(r.text);
    expect(j.expiresAt - Date.now()).toBeGreaterThan(C.ENROLL_TTL_MS - 60_000);
    expect(j.expiresAt - Date.now()).toBeLessThanOrEqual(C.ENROLL_TTL_MS);
    expect((await enroll(j.code, 'ttl bot')).status).toBe(201);
    expect((await enroll(j.code, 'ttl bot two')).status).toBe(401);
  });
});

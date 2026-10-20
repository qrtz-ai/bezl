// Local test mode (src/dev.ts). The test pool runs without DASH_DEV_LOCAL, i.e. exactly like production.
import { env, exports } from 'cloudflare:workers';
import { describe, expect, it } from 'vitest';
import { devLocal, TEST_CODE_PREFIX } from '../src/dev';
import type { Env } from '../src/env';

const e = (v?: string) => ({ ...(env as unknown as Env), DASH_DEV_LOCAL: v });
const rq = (
  url: string,
  ip: string | null,
  extra: Record<string, string> = {},
) =>
  new Request(url, {
    headers: { ...(ip ? { 'cf-connecting-ip': ip } : {}), ...extra },
  });

describe('local test mode gate', () => {
  it('needs the var, a loopback peer and host 127.0.0.1', () => {
    expect(devLocal(e('1'), rq('http://127.0.0.1:8799/', '127.0.0.1'))).toBe(
      true,
    );
    expect(devLocal(e('1'), rq('http://127.0.0.1:8799/', '::1'))).toBe(true);
    expect(
      devLocal(e(undefined), rq('http://127.0.0.1:8799/', '127.0.0.1')),
    ).toBe(false);
    expect(devLocal(e('true'), rq('http://127.0.0.1:8799/', '127.0.0.1'))).toBe(
      false,
    );
    // a real (non-loopback) client, or none
    expect(devLocal(e('1'), rq('http://127.0.0.1:8799/', '203.0.113.9'))).toBe(
      false,
    );
    expect(devLocal(e('1'), rq('http://127.0.0.1:8799/', null))).toBe(false);
    // other hosts, including look-alikes and localhost
    for (const u of [
      'https://dash.qrtz.ai/',
      'http://localhost:8799/',
      'http://127.0.0.1.evil.example/',
      'http://10.0.0.1/',
    ])
      expect(devLocal(e('1'), rq(u, '127.0.0.1')), u).toBe(false);
  });

  it('no request header turns it on', () => {
    expect(
      devLocal(
        e(undefined),
        rq('http://127.0.0.1:8799/', '127.0.0.1', {
          'x-dash-dev-local': '1',
          'dash-dev-local': '1',
          host: '127.0.0.1',
        }),
      ),
    ).toBe(false);
  });

  it('the production config cannot set DASH_DEV_LOCAL', () => {
    expect(env.TEST_WRANGLER_TOML).not.toMatch(/DASH_DEV_LOCAL/);
    expect(env.TEST_WRANGLER_TOML).not.toMatch(/^\s*\[(env\.[^\]]+\.)?vars\]/m);
    expect((env as unknown as Env).DASH_DEV_LOCAL).toBeUndefined();
  });

  it('a production build answers plain http with 400, even from loopback', async () => {
    const r = await exports.default.fetch(
      rq('http://127.0.0.1:8799/v1/readme', '127.0.0.1'),
    );
    expect(r.status).toBe(400);
  });

  it('production refuses a test-prefixed code with the same answer as a wrong code', async () => {
    const post = (code: string, ip: string) =>
      exports.default.fetch(
        new Request('https://dash.qrtz.ai/v1/enroll', {
          method: 'POST',
          headers: {
            'content-type': 'application/json',
            'cf-connecting-ip': ip,
          },
          body: JSON.stringify({ code, name: 'x' }),
        }),
      );
    // QRT-284 format: a well-formed TST- code takes the refused path, not the malformed one.
    const tst = await post(
      `${TEST_CODE_PREFIX}ABCDEFGH.${'2'.repeat(17)}`,
      '192.0.2.201',
    );
    const wrong = await post(`ENR-ABCDEFGH.${'2'.repeat(17)}`, '192.0.2.202');
    expect(tst.status).toBe(401);
    expect(await tst.text()).toBe(await wrong.text());
  });
});

describe('test-code endpoint and locked sources (item 3)', () => {
  it('POST /v1/owner/test-codes is 404 in production, with or without an owner key', async () => {
    for (const headers of <Record<string, string>[]>[
      {},
      { authorization: 'Bearer ok_' + '0'.repeat(32) + '_' + '0'.repeat(64) },
    ]) {
      const r = await exports.default.fetch(
        new Request('https://dash.qrtz.ai/v1/owner/test-codes', {
          method: 'POST',
          headers: { 'cf-connecting-ip': '203.0.113.50', ...headers },
        }),
      );
      expect(r.status).toBe(404);
    }
  });

  it('a locked source gets the same 429 for a TST- code as for a normal code', async () => {
    const ip = '192.0.2.233';
    const post = (code: string) =>
      exports.default.fetch(
        new Request('https://dash.qrtz.ai/v1/enroll', {
          method: 'POST',
          headers: {
            'content-type': 'application/json',
            'cf-connecting-ip': ip,
          },
          body: JSON.stringify({ code, name: 'x' }),
        }),
      );
    let last = 0;
    for (let i = 0; i < 12; i++)
      last = (await post(`ENR-AAAA-AAA${i % 10}`)).status;
    expect(last).toBe(429);
    const tst = await post(`${TEST_CODE_PREFIX}ABCDEFGH.${'2'.repeat(17)}`);
    const enr = await post('ENR-ZZZZ-ZZZZ');
    expect(tst.status).toBe(429);
    expect(enr.status).toBe(429);
  });
});

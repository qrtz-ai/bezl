// Local test mode (src/dev.ts). The test pool runs without DASH_DEV_LOCAL, i.e. exactly like production.
import { env, exports } from 'cloudflare:workers';
import { describe, expect, it } from 'vitest';
import {
  devLocal,
  devOrigin,
  README_RULE_1,
  readmeFor,
  TEST_CODE_PREFIX,
} from '../src/dev';
import { README_MD } from '../src/readme.gen';
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
    const tst = await post(`${TEST_CODE_PREFIX}ABCD-EFGH`, '192.0.2.201');
    const wrong = await post('ENR-ABCD-EFGH', '192.0.2.202');
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
    const tst = await post(`${TEST_CODE_PREFIX}ABCD-EFGH`);
    const enr = await post('ENR-ZZZZ-ZZZZ');
    expect(tst.status).toBe(429);
    expect(enr.status).toBe(429);
  });
});

describe('QRT-298: readme rule 1 names the origin', () => {
  const dev = (o?: string) => ({ ...e('1'), DASH_DEV_ORIGIN: o });
  const local = rq('http://127.0.0.1:8799/v1/readme', '127.0.0.1');

  it('production: rule 1 is https://dash.qrtz.ai, unchanged', async () => {
    expect(README_MD).toContain(README_RULE_1);
    const r = await exports.default.fetch('https://dash.qrtz.ai/v1/readme');
    const t = await r.text();
    expect(t).toBe(README_MD);
    expect(t).toContain(README_RULE_1);
    expect(t).not.toMatch(/127\.0\.0\.1/);
  });

  it('dev-local: rule 1 names the configured local origin', () => {
    const o = devOrigin(dev('http://127.0.0.1:8799'), local);
    expect(o).toBe('http://127.0.0.1:8799');
    const t = readmeFor(README_MD, o);
    expect(t).not.toContain(README_RULE_1);
    expect(t).toContain(
      '1. One origin: `http://127.0.0.1:8799` (a local test server). Never send tokens anywhere else.',
    );
  });

  it('only from the var, only in local test mode, only loopback http', () => {
    // not local test mode (production): the var is ignored
    expect(
      devOrigin(
        { ...e(undefined), DASH_DEV_ORIGIN: 'http://127.0.0.1:8799' },
        local,
      ),
    ).toBeNull();
    expect(
      devOrigin(
        dev('http://127.0.0.1:8799'),
        rq('https://dash.qrtz.ai/v1/readme', '127.0.0.1'),
      ),
    ).toBeNull();
    // no var: the Host header never stands in for it
    expect(
      devOrigin(
        dev(undefined),
        rq('http://127.0.0.1:8799/v1/readme', '127.0.0.1', {
          host: 'evil.example',
        }),
      ),
    ).toBeNull();
    for (const o of [
      'https://dash.qrtz.ai',
      'http://localhost:8799',
      'http://127.0.0.1',
      'http://127.0.0.1:0',
      'http://127.0.0.1:65536',
      'http://127.0.0.1:8799/',
      'http://evil.example',
    ])
      expect(devOrigin(dev(o), local), o).toBeNull();
  });

  it('the production config cannot set DASH_DEV_ORIGIN', () => {
    expect(env.TEST_WRANGLER_TOML).not.toMatch(/DASH_DEV_ORIGIN/);
    expect((env as unknown as Env).DASH_DEV_ORIGIN).toBeUndefined();
  });
});

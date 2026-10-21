import { env, exports } from 'cloudflare:workers';
import { runDurableObjectAlarm, runInDurableObject } from 'cloudflare:test';
import { describe, expect, it } from 'vitest';
import * as C from '../src/constants';
import type { Dashboard } from '../src/dashboard';
import type { EnrollDirectory } from '../src/directory';
import { clearTombCache } from '../src/index';
import { sourceOf } from '../src/ip';
import { routeTemplate } from '../src/reqlog';

const O = 'https://dash.qrtz.ai';
type J = Record<string, any>;
let ipN = 0;

const call = async (
  method: string,
  path: string,
  opts: {
    bearer?: string;
    body?: unknown;
    raw?: string;
    headers?: Record<string, string>;
    ip?: string;
  } = {},
) => {
  const headers: Record<string, string> = {
    'content-type': 'application/json',
    'cf-connecting-ip': opts.ip ?? '203.0.113.1',
    ...opts.headers,
  };
  if (opts.bearer) headers.authorization = `Bearer ${opts.bearer}`;
  const body =
    opts.raw ??
    (opts.body === undefined ? undefined : JSON.stringify(opts.body));
  const r = await exports.default.fetch(
    new Request(O + path, { method, headers, body }),
  );
  const text = await r.text();
  let json: J = {};
  try {
    json = JSON.parse(text);
  } catch {}
  return { status: r.status, json, text, headers: r.headers };
};

/** Test seam: clear the one-time setup flag so each test can make its own dashboards. */
const resetSetup = () =>
  runInDurableObject(
    env.ENROLL.getByName('enroll'),
    (_i: EnrollDirectory, state) => {
      state.storage.sql.exec("DELETE FROM meta WHERE k = 'setup_used'");
    },
  );
const failureRows = () =>
  runInDurableObject(
    env.ENROLL.getByName('enroll'),
    (_i: EnrollDirectory, state) =>
      state.storage.sql
        .exec<{ n: number }>('SELECT COUNT(*) AS n FROM failures')
        .one().n,
  );
const setup = async () => {
  await resetSetup();
  const r = await call('POST', '/v1/owner/setup', {
    bearer: env.DASH_SETUP_TOKEN,
  });
  expect(r.status).toBe(201);
  return {
    dashId: r.json.dashboard_id as string,
    owner: r.json.owner_key as string,
  };
};
const issue = async (owner: string) => {
  const r = await call('POST', '/v1/owner/codes', { bearer: owner });
  expect(r.status).toBe(201);
  return r.json.code as string;
};
const enroll = async (
  owner: string,
  name = 'grok bot',
  ip = `198.51.100.${++ipN}`,
) => {
  const code = await issue(owner);
  const r = await call('POST', '/v1/enroll', {
    body: { code, name, description: "jared's assistant" },
    ip,
  });
  expect(r.status, JSON.stringify(r.json)).toBe(201);
  return {
    id: r.json.agent_id as string,
    token: r.json.agent_token as string,
    fingerprint: r.json.fingerprint as string,
    code,
  };
};
const approved = async (owner: string, name?: string) => {
  const a = await enroll(owner, name);
  const r = await call('POST', `/v1/owner/agents/${a.id}/review`, {
    bearer: owner,
    body: { approve: true },
  });
  expect(r.json.ok).toBe(true);
  return a;
};
const view = async (owner: string) =>
  (await call('GET', '/v1/owner/view', { bearer: owner })).json;

describe('readme and transport', () => {
  it('serves the static agent readme, https only, with hsts', async () => {
    const r = await call('GET', '/v1/readme');
    expect(r.status).toBe(200);
    expect(r.text).toContain('POST /v1/enroll');
    expect(r.text).not.toContain('Server design');
    expect(r.headers.get('strict-transport-security')).toContain('max-age=');
    const h = await exports.default.fetch('http://dash.qrtz.ai/v1/readme');
    expect(h.status).toBe(400);
  });
});

describe('setup and the owner key', () => {
  it('needs the setup token, and the owner key reads only its own dashboard', async () => {
    expect(
      (await call('POST', '/v1/owner/setup', { bearer: 'nope' })).status,
    ).toBe(401);
    const a = await setup();
    const b = await setup();
    expect(a.dashId).not.toBe(b.dashId);
    expect(
      (await call('GET', '/v1/owner/view', { bearer: a.owner })).status,
    ).toBe(200);
    // Right dashboard id, wrong secret.
    const forged = `ok_${a.dashId}_${'0'.repeat(64)}`;
    expect(
      (await call('GET', '/v1/owner/view', { bearer: forged })).status,
    ).toBe(401);
    // Never from the query string.
    expect((await call('GET', `/v1/owner/view?key=${a.owner}`)).status).toBe(
      401,
    );
  });
  it('setup is one-time: a second use of the token is 410, even with the right token (M1)', async () => {
    await setup();
    const again = await call('POST', '/v1/owner/setup', {
      bearer: env.DASH_SETUP_TOKEN,
    });
    expect(again.status).toBe(410);
    expect(again.json.owner_key).toBeUndefined();
    // A wrong token still gets 401, not a hint that setup happened.
    expect(
      (await call('POST', '/v1/owner/setup', { bearer: 'nope' })).status,
    ).toBe(401);
    // Concurrent uses: exactly one wins.
    await resetSetup();
    const rs = await Promise.all(
      Array.from({ length: 5 }, () =>
        call('POST', '/v1/owner/setup', { bearer: env.DASH_SETUP_TOKEN }),
      ),
    );
    expect(rs.map((r) => r.status).sort()).toEqual([201, 410, 410, 410, 410]);
  });
  it('a malformed %-escape in an owner path is 404, not 500 (L5)', async () => {
    const { owner } = await setup();
    for (const path of [
      '/v1/owner/agents/%E0%A4%A/review',
      '/v1/owner/agents/%/review',
      '/v1/owner/widgets/%ZZ',
    ]) {
      const r = await call('POST', path, {
        bearer: owner,
        body: { approve: true },
      });
      expect(r.status, path).toBe(404);
    }
  });
  it('stores only HMACs at rest', async () => {
    const { dashId, owner } = await setup();
    const ag = await enroll(owner);
    const dump = await runInDurableObject(
      env.DASHBOARD.getByName(dashId),
      (i: Dashboard) => i.dumpForTest(),
    );
    const s = JSON.stringify(dump);
    expect(s).not.toContain(owner);
    expect(s).not.toContain(ag.token);
    expect(s).not.toContain(ag.code);
    const v = JSON.stringify(await view(owner));
    expect(v).not.toContain(ag.token);
    expect(v).not.toContain('token_hash');
  });
});

describe('enrollment', () => {
  it('a code is single use, expires, makes a pending agent, and is never a write bearer', async () => {
    const { owner } = await setup();
    const code = await issue(owner);
    const body = { code, name: 'grok bot', description: "jared's assistant" };
    const r = await call('POST', '/v1/enroll', { body, ip: '192.0.2.10' });
    expect(r.status).toBe(201);
    expect(r.json).toMatchObject({
      status: 'pending',
      readme_version: C.README_VERSION,
    });
    expect(r.json.agent_token).toMatch(/^at_[0-9a-f]{32}_[0-9a-f]{64}$/);
    expect(r.json.fingerprint).toMatch(/^[0-9A-F]{4}-[0-9A-F]{4}$/);
    expect(
      (await call('POST', '/v1/enroll', { body, ip: '192.0.2.10' })).status,
    ).toBe(401);
    expect(
      (
        await call('POST', '/v1/heartbeat', {
          bearer: code,
          body: { status: 'idle' },
        })
      ).status,
    ).toBe(401);
    // pending: 403 until approved
    expect(
      (
        await call('POST', '/v1/heartbeat', {
          bearer: r.json.agent_token,
          body: { status: 'idle' },
        })
      ).status,
    ).toBe(403);
    // re-enrolling with the same name while one is live: 409; reserved: 422
    const c2 = await issue(owner);
    expect(
      (
        await call('POST', '/v1/enroll', {
          body: { code: c2, name: 'Grok Bot' },
          ip: '192.0.2.11',
        })
      ).status,
    ).toBe(409);
    const c3 = await issue(owner);
    expect(
      (
        await call('POST', '/v1/enroll', {
          body: { code: c3, name: 'Sуstеm' },
          ip: '192.0.2.11',
        })
      ).status,
    ).toBe(422);
  });

  it('an expired code is refused', async () => {
    const { dashId, owner } = await setup();
    const code = await issue(owner);
    await runInDurableObject(
      env.DASHBOARD.getByName(dashId),
      (_i: Dashboard, state) => {
        state.storage.sql.exec(
          'UPDATE enroll_code_macs SET expires_at = ?',
          Date.now() - 1,
        );
      },
    );
    expect(
      (
        await call('POST', '/v1/enroll', {
          body: { code, name: 'x bot' },
          ip: '192.0.2.20',
        })
      ).status,
    ).toBe(401);
  });

  it('rotation kills outstanding codes, not tokens', async () => {
    const { owner } = await setup();
    const a = await approved(owner);
    const code = await issue(owner);
    expect(
      (await call('POST', '/v1/owner/codes/rotate', { bearer: owner })).status,
    ).toBe(200);
    expect(
      (
        await call('POST', '/v1/enroll', {
          body: { code, name: 'late bot' },
          ip: '192.0.2.30',
        })
      ).status,
    ).toBe(401);
    expect(
      (
        await call('POST', '/v1/heartbeat', {
          bearer: a.token,
          body: { status: 'idle' },
        })
      ).status,
    ).toBe(200);
  });

  it('wrong codes: 10 a minute per ip, then 429; 50 a day, then locked out', async () => {
    const ip = '192.0.2.40';
    for (let i = 0; i < C.ENROLL_IP_FAILS_PER_MIN; i++)
      expect(
        (
          await call('POST', '/v1/enroll', {
            body: { code: `ENR-AAAA-AA${i}`, name: 'x' },
            ip,
          })
        ).status,
      ).toBe(401);
    const r = await call('POST', '/v1/enroll', {
      body: { code: 'ENR-AAAA-BBBB', name: 'x' },
      ip,
    });
    expect(r.status).toBe(429);
    expect(r.json.retry_after_ms).toBeGreaterThan(0);
    expect(r.headers.get('retry-after')).not.toBeNull();
    // A good code from a fresh ip still works.
    const { owner } = await setup();
    await enroll(owner, 'other bot', '192.0.2.41');
    // Lockout after 50 in a day (failures recorded straight into the directory, as the Worker would).
    const dir = env.ENROLL.getByName('enroll');
    const lockIp = { narrow: 'iphash-test-lock', wide: null };
    for (let i = 0; i < C.ENROLL_IP_FAILS_BEFORE_LOCK; i++)
      await dir.fail(lockIp, Date.now() - 2 * 60_000 - i);
    const c = await dir.claim(lockIp, 'whatever', 'f'.repeat(64));
    expect(c).toMatchObject({ ok: false, status: 429 });
    expect(c.ok === false && (c.retryAfterMs ?? 0) > 60 * 60_000).toBe(true);
  });

  it('limiter buckets: ipv4 /32, ipv6 /64 with a /48 count, mapped v4 is v4, junk refused (M2)', () => {
    expect(sourceOf('203.0.113.7')).toEqual({
      narrow: 'v4:203.0.113.7',
      wide: null,
    });
    expect(sourceOf('2001:db8:1:2:aaaa::1')).toEqual({
      narrow: 'v6/64:2001:db8:1:2',
      wide: 'v6/48:2001:db8:1',
    });
    // Same /64 however it is written.
    expect(sourceOf('2001:DB8:1:2:ffff:ffff:ffff:ffff')?.narrow).toBe(
      sourceOf('2001:db8:1:2::')?.narrow,
    );
    expect(sourceOf('2001:db8:1:3::1')?.wide).toBe(
      sourceOf('2001:db8:1:2::1')?.wide,
    );
    expect(sourceOf('::ffff:192.0.2.9')).toEqual({
      narrow: 'v4:192.0.2.9',
      wide: null,
    });
    expect(sourceOf('::1')?.narrow).toBe('v6/64:0:0:0:0');
    for (const bad of [
      null,
      '',
      'unknown',
      '1.2.3',
      '256.1.1.1',
      '01.2.3.4',
      '1::2::3',
      '1:2:3:4:5:6:7:8:9',
      'fe80::1%eth0',
      'g::1',
    ])
      expect(sourceOf(bad), String(bad)).toBeNull();
  });

  it('no or junk CF-Connecting-IP is refused, never pooled as unknown (M2)', async () => {
    const { owner } = await setup();
    const code = await issue(owner);
    const before = await failureRows();
    for (const ip of [undefined, '', 'unknown']) {
      const headers: Record<string, string> = {
        'content-type': 'application/json',
      };
      if (ip !== undefined) headers['cf-connecting-ip'] = ip;
      const r = await exports.default.fetch(
        new Request(O + '/v1/enroll', {
          method: 'POST',
          headers,
          body: JSON.stringify({ code, name: 'x bot' }),
        }),
      );
      expect(r.status, String(ip)).toBe(400);
    }
    // Nothing was counted, and the code is still good from a real address.
    expect(await failureRows()).toBe(before);
    const ok = await call('POST', '/v1/enroll', {
      body: { code, name: 'x bot' },
      ip: '192.0.2.70',
    });
    expect(ok.status).toBe(201);
  });

  it('ipv6: rotating inside a /64 is one bucket; many /64s in one /48 hit the /48 count (M2)', async () => {
    const wrong = (ip: string) =>
      call('POST', '/v1/enroll', {
        body: { code: 'ENR-AAAA-AAAA', name: 'x' },
        ip,
      });
    // One /64, a new address every try.
    for (let i = 0; i < C.ENROLL_IP_FAILS_PER_MIN; i++)
      expect(
        (await wrong(`2001:db8:a:1::${(i + 1).toString(16)}`)).status,
      ).toBe(401);
    expect((await wrong('2001:db8:a:1:dead:beef:0:1')).status).toBe(429);
    // Other /64s in the same /48 until the looser /48 count is spent.
    let n = C.ENROLL_IP_FAILS_PER_MIN;
    for (let net = 2; n < C.ENROLL_V6_48_FAILS_PER_MIN; net++)
      for (
        let i = 0;
        i < C.ENROLL_IP_FAILS_PER_MIN && n < C.ENROLL_V6_48_FAILS_PER_MIN;
        i++, n++
      )
        expect(
          (await wrong(`2001:db8:a:${net.toString(16)}::${i + 1}`)).status,
        ).toBe(401);
    expect((await wrong('2001:db8:a:ff::1')).status).toBe(429);
    // A different /48 and an ipv4 client are untouched.
    const { owner } = await setup();
    await enroll(owner, 'v6 bot', '2001:db8:b:1::1');
    await enroll(owner, 'v4 bot', '192.0.2.60');
  });

  it('ipv6 lockout covers the whole /64 and the /48 (M2)', async () => {
    const dir = env.ENROLL.getByName('enroll');
    const a = { narrow: 'h64-a', wide: 'h48-x' };
    for (let i = 0; i < C.ENROLL_IP_FAILS_BEFORE_LOCK; i++)
      await dir.fail(a, Date.now() - 2 * 60_000 - i);
    expect(await dir.claim(a, 'whatever', 'f'.repeat(64))).toMatchObject({
      ok: false,
      status: 429,
    });
    // Another /64 in the same /48 is fine until the /48 itself reaches its lock count.
    const b = { narrow: 'h64-b', wide: 'h48-x' };
    expect(await dir.claim(b, 'whatever', 'f'.repeat(64))).toMatchObject({
      ok: false,
      status: 401,
    });
    for (let i = 0; i < C.ENROLL_V6_48_FAILS_BEFORE_LOCK; i++)
      await dir.fail(
        { narrow: `h64-${i}`, wide: 'h48-y' },
        Date.now() - 2 * 60_000 - i,
      );
    const c = await dir.claim(
      { narrow: 'h64-fresh', wide: 'h48-y' },
      'whatever',
      'f'.repeat(64),
    );
    expect(c).toMatchObject({ ok: false, status: 429 });
    expect(c.ok === false && (c.retryAfterMs ?? 0) > 60 * 60_000).toBe(true);
  });

  it('one attacker cannot trip the service-wide brake (M2)', async () => {
    const wrong = (ip: string) =>
      call('POST', '/v1/enroll', {
        body: { code: 'ENR-AAAA-AAAA', name: 'x' },
        ip,
      });
    // One /48 tries far more than the brake, over thousands of addresses.
    const before = await failureRows();
    let refused = 0;
    for (let i = 0; i < C.ENROLL_V6_48_FAILS_PER_MIN * 3; i++)
      if (
        (
          await wrong(
            `2001:db8:c:${(i % 200).toString(16)}::${(i + 1).toString(16)}`,
          )
        ).status === 429
      )
        refused++;
    expect(refused).toBe(C.ENROLL_V6_48_FAILS_PER_MIN * 2);
    // Only what the /48 cap let through reached the brake's count.
    expect((await failureRows()) - before).toBe(C.ENROLL_V6_48_FAILS_PER_MIN);
    const { owner } = await setup();
    await enroll(owner, 'bystander', '198.51.100.200');
    // The brake still exists for a real distributed flood.
    await runInDurableObject(
      env.ENROLL.getByName('enroll'),
      (_i: EnrollDirectory, state) => {
        for (let i = 0; i < C.ENROLL_GLOBAL_FAILS_PER_MIN; i++)
          state.storage.sql.exec(
            'INSERT INTO failures (narrow, wide, at) VALUES (?, NULL, ?)',
            `flood-${i}`,
            Date.now(),
          );
      },
    );
    const { owner: o2 } = await setup();
    const code = await issue(o2);
    expect(
      (
        await call('POST', '/v1/enroll', {
          body: { code, name: 'z bot' },
          ip: '198.51.100.201',
        })
      ).status,
    ).toBe(429);
    await runInDurableObject(
      env.ENROLL.getByName('enroll'),
      (_i: EnrollDirectory, state) => {
        state.storage.sql.exec(
          "DELETE FROM failures WHERE narrow LIKE 'flood-%'",
        );
      },
    );
  });

  it('dashboard-wide: 50 attributed wrong codes force a rotation', async () => {
    const { dashId, owner } = await setup();
    const code = await issue(owner);
    await runInDurableObject(
      env.DASHBOARD.getByName(dashId),
      async (i: Dashboard, state) => {
        state.storage.sql.exec(
          "INSERT INTO meta (k, v) VALUES ('enroll_fail_total', ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v",
          String(C.ENROLL_FAILS_BEFORE_ROTATION - 1),
        );
        const r = await i.enroll({
          codeId: 'ENR-NOSUCHID',
          secretMac: 'f'.repeat(64),
          name: 'x',
          description: '',
          tokenHash: 'a'.repeat(64),
        });
        expect(r.status).toBe(401);
      },
    );
    expect(
      (
        await call('POST', '/v1/enroll', {
          body: { code, name: 'y bot' },
          ip: '192.0.2.50',
        })
      ).status,
    ).toBe(401);
    const v = await view(owner);
    expect(v.audit.map((a: J) => a.event)).toContain(
      'codes-rotated-after-failures',
    );
  });
});

describe('writes', () => {
  it('auth comes before json parsing, and tokens only count from the header', async () => {
    expect(
      (await call('PUT', '/v1/widgets/x', { raw: '{not json' })).status,
    ).toBe(401);
    const { owner } = await setup();
    const a = await enroll(owner);
    // pending agent + bad json: still the auth answer (403), not 422
    expect(
      (
        await call('PUT', '/v1/widgets/x', {
          bearer: a.token,
          raw: '{not json',
        })
      ).status,
    ).toBe(403);
    expect(
      (
        await call('POST', `/v1/heartbeat?token=${a.token}`, {
          body: { status: 'idle' },
        })
      ).status,
    ).toBe(401);
    const forged = a.token.slice(0, -4) + '0000';
    expect(
      (
        await call('POST', '/v1/heartbeat', {
          bearer: forged,
          body: { status: 'idle' },
        })
      ).json.action,
    ).toBe('stop');
  });

  it('register, put, append, batch, typed blocks, 409/422/404', async () => {
    const { owner } = await setup();
    const a = await approved(owner);
    const t = a.token;
    expect(
      (
        await call('POST', '/v1/widgets', {
          bearer: t,
          body: {
            key: 'state-farm',
            type: 'status',
            title: 'State Farm CA policy',
          },
        })
      ).status,
    ).toBe(201);
    const again = await call('POST', '/v1/widgets', {
      bearer: t,
      body: { key: 'state-farm', type: 'status' },
    });
    expect(again.status).toBe(201);
    expect(
      (
        await call('POST', '/v1/widgets', {
          bearer: t,
          body: { key: 'state-farm', type: 'log' },
        })
      ).status,
    ).toBe(409);
    expect(
      (
        await call('POST', '/v1/widgets', {
          bearer: t,
          body: { key: 'x', type: 'action' },
        })
      ).status,
    ).toBe(422);
    expect(
      (
        await call('POST', '/v1/widgets', {
          bearer: t,
          body: { key: 'Bad', type: 'log' },
        })
      ).status,
    ).toBe(422);
    expect(
      (
        await call('POST', '/v1/widgets', {
          bearer: t,
          body: { key: 'activity', type: 'log' },
        })
      ).status,
    ).toBe(201);
    expect(
      (
        await call('PUT', '/v1/widgets/state-farm', {
          bearer: t,
          body: { status: 'blocked', text: 'Waiting on User ID' },
        })
      ).status,
    ).toBe(200);
    expect(
      (
        await call('PUT', '/v1/widgets/state-farm', {
          bearer: t,
          body: { status: 'needs_you' },
        })
      ).status,
    ).toBe(422);
    expect(
      (
        await call('POST', '/v1/widgets/activity/events', {
          bearer: t,
          body: { line: 'Uploaded bank statement', level: 'info' },
        })
      ).status,
    ).toBe(200);
    expect(
      (
        await call('PUT', '/v1/widgets/activity', {
          bearer: t,
          body: { line: 'x' },
        })
      ).status,
    ).toBe(422);
    const nf = await call('PUT', '/v1/widgets/nope', {
      bearer: t,
      body: { status: 'done' },
    });
    expect(nf.status).toBe(404);
    expect(nf.json.action).toBe('register_own_widget');
    const b = await call('POST', '/v1/batch', {
      bearer: t,
      body: {
        ops: [
          { op: 'put', key: 'state-farm', data: { status: 'done' } },
          { op: 'append', key: 'activity', data: { line: 'two' } },
          { op: 'put', key: 'nope', data: {} },
          { op: 'drop', key: 'activity' },
        ],
      },
    });
    expect(b.status).toBe(200);
    expect(b.json.results.map((r: J) => r.status)).toEqual([
      200, 200, 404, 422,
    ]);
    expect(b.json.results[0].result.readme_version).toBeUndefined();
    const tooMany = Array.from({ length: C.BATCH_MAX_OPS + 1 }, () => ({
      op: 'put',
      key: 'state-farm',
      data: { status: 'done' },
    }));
    expect(
      (await call('POST', '/v1/batch', { bearer: t, body: { ops: tooMany } }))
        .status,
    ).toBe(422);
    expect(
      (await call('POST', '/v1/batch', { bearer: t, body: { ops: [] } }))
        .status,
    ).toBe(422);
    const v = await view(owner);
    const ws = v.agents[0].widgets;
    expect(ws.find((w: J) => w.key === 'state-farm').state).toEqual({
      status: 'done',
    });
    expect(
      ws.find((w: J) => w.key === 'activity').events.map((e: J) => e.data.line),
    ).toEqual(['Uploaded bank statement', 'two']);
  });

  it('another agent cannot write my widget', async () => {
    const { owner } = await setup();
    const a = await approved(owner, 'a bot');
    const b = await approved(owner, 'b bot');
    await call('POST', '/v1/widgets', {
      bearer: a.token,
      body: { key: 'mine', type: 'status' },
    });
    const r = await call('PUT', '/v1/widgets/mine', {
      bearer: b.token,
      body: { status: 'done' },
    });
    expect(r.status).toBe(404);
  });

  it('body caps from content-length and the stream', async () => {
    const { owner } = await setup();
    const a = await approved(owner);
    await call('POST', '/v1/widgets', {
      bearer: a.token,
      body: { key: 'm', type: 'markdown' },
    });
    const big = JSON.stringify({ text: 'x'.repeat(C.MAX_BODY_BYTES) });
    expect(
      (await call('PUT', '/v1/widgets/m', { bearer: a.token, raw: big }))
        .status,
    ).toBe(413);
    expect(
      (
        await call('PUT', '/v1/widgets/m', {
          bearer: a.token,
          raw: '{}',
          headers: { 'content-length': '999999' },
        })
      ).status,
    ).toBe(413);
    // Streamed without a length.
    const stream = new ReadableStream({
      start(c) {
        c.enqueue(new TextEncoder().encode(big));
        c.close();
      },
    });
    const r = await exports.default.fetch(
      new Request(`${O}/v1/widgets/m`, {
        method: 'PUT',
        headers: { authorization: `Bearer ${a.token}` },
        body: stream,
        duplex: 'half',
      } as RequestInit),
    );
    expect(r.status).toBe(413);
  });

  it('idempotency: the same key or seq never double-appends', async () => {
    const { owner } = await setup();
    const a = await approved(owner);
    await call('POST', '/v1/widgets', {
      bearer: a.token,
      body: { key: 'log', type: 'log' },
    });
    const h = { 'idempotency-key': '6f1c3d0e-0000-4000-8000-000000000001' };
    for (let i = 0; i < 3; i++)
      await call('POST', '/v1/widgets/log/events', {
        bearer: a.token,
        body: { line: 'once' },
        headers: h,
      });
    for (let i = 0; i < 3; i++)
      await call('POST', '/v1/widgets/log/events', {
        bearer: a.token,
        body: { line: 'seq', seq: 7 },
      });
    const ev = (await view(owner)).agents[0].widgets[0].events;
    expect(ev.map((e: J) => e.data.line)).toEqual(['once', 'seq']);
    expect(ev[1].data.seq).toBeUndefined();
  });

  it('rate limit: 60 writes a minute per agent, then 429 with retry_after_ms', async () => {
    const { owner } = await setup();
    const a = await approved(owner);
    for (let i = 0; i < C.WRITES_PER_MIN_AGENT; i++)
      expect(
        (
          await call('POST', '/v1/heartbeat', {
            bearer: a.token,
            body: { status: 'running' },
          })
        ).status,
      ).toBe(200);
    const r = await call('POST', '/v1/heartbeat', {
      bearer: a.token,
      body: { status: 'running' },
    });
    expect(r.status).toBe(429);
    expect(r.json.retry_after_ms).toBeGreaterThanOrEqual(1000);
  });

  it('the scanner redacts and flags before storage', async () => {
    const { owner } = await setup();
    const a = await approved(owner);
    await call('POST', '/v1/widgets', {
      bearer: a.token,
      body: { key: 's', type: 'status' },
    });
    await call('PUT', '/v1/widgets/s', {
      bearer: a.token,
      body: { status: 'running', text: 'see\nghp_abcdefghijklmnopqrstuvwx' },
    });
    const ag = (await view(owner)).agents[0];
    expect(ag.flagged).toBe(true);
    expect(ag.widgets[0].state.text).toBe('see\n[redacted]');
  });
});

describe('tombstones', () => {
  it('widget, agent and dashboard deletes answer 410 action stop with their scope', async () => {
    clearTombCache();
    const { owner } = await setup();
    const a = await approved(owner);
    await call('POST', '/v1/widgets', {
      bearer: a.token,
      body: { key: 'w', type: 'status' },
    });
    expect(
      (await call('DELETE', `/v1/owner/widgets/${a.id}/w`, { bearer: owner }))
        .json.ok,
    ).toBe(true);
    const w = await call('PUT', '/v1/widgets/w', {
      bearer: a.token,
      body: { status: 'done' },
    });
    expect(w.status).toBe(410);
    expect(w.json).toMatchObject({ action: 'stop', scope: 'widget' });
    expect(
      (
        await call('POST', '/v1/widgets', {
          bearer: a.token,
          body: { key: 'w', type: 'status' },
        })
      ).status,
    ).toBe(410);
    expect(
      (await call('DELETE', `/v1/owner/agents/${a.id}`, { bearer: owner })).json
        .ok,
    ).toBe(true);
    for (let i = 0; i < 2; i++) {
      // second time from the Worker's cache
      const r = await call('POST', '/v1/heartbeat', {
        bearer: a.token,
        body: { status: 'idle' },
      });
      expect(r.status).toBe(410);
      expect(r.json.scope).toBe('agent');
    }
    const b = await approved(owner, 'b bot');
    expect(
      (await call('DELETE', '/v1/owner/dashboard', { bearer: owner })).status,
    ).toBe(200);
    const d = await call('POST', '/v1/heartbeat', {
      bearer: b.token,
      body: { status: 'idle' },
    });
    expect(d.status).toBe(410);
    expect(d.json.scope).toBe('dashboard');
    expect(
      (await call('POST', '/v1/owner/codes', { bearer: owner })).status,
    ).toBe(422);
    expect((await view(owner)).deleted).toBe(true);
  });

  it('a rejected agent answers 401 stop and leaves the view', async () => {
    const { owner } = await setup();
    const a = await enroll(owner);
    await call('POST', `/v1/owner/agents/${a.id}/review`, {
      bearer: owner,
      body: { approve: false },
    });
    const r = await call('POST', '/v1/heartbeat', {
      bearer: a.token,
      body: { status: 'idle' },
    });
    expect(r.status).toBe(401);
    expect(r.json.action).toBe('stop');
    expect((await view(owner)).agents).toEqual([]);
  });
});

describe('stale and retention', () => {
  it('stale after 10 minutes quiet; 30-day retention runs from the alarm', async () => {
    const { dashId, owner } = await setup();
    const a = await approved(owner);
    await call('POST', '/v1/widgets', {
      bearer: a.token,
      body: { key: 'c', type: 'chart' },
    });
    await call('POST', '/v1/widgets/c/events', {
      bearer: a.token,
      body: { value: 1 },
    });
    const stub = env.DASHBOARD.getByName(dashId);
    const later = await runInDurableObject(stub, (i: Dashboard) =>
      i.view(Date.now() + C.STALE_AFTER_MS + 1000),
    );
    expect(later.agents[0]?.stale).toBe(true);
    const now = await runInDurableObject(stub, (i: Dashboard) =>
      i.view(Date.now()),
    );
    expect(now.agents[0]?.stale).toBe(false);
    await runInDurableObject(stub, (_i: Dashboard, state) => {
      state.storage.sql.exec(
        'UPDATE events SET at = ?',
        Date.now() - (C.RETENTION_DAYS + 1) * C.DAY_MS,
      );
    });
    expect(await runDurableObjectAlarm(stub)).toBe(true);
    const n = await runInDurableObject(
      stub,
      (_i: Dashboard, state) =>
        state.storage.sql
          .exec<{ n: number }>('SELECT COUNT(*) AS n FROM events')
          .one().n,
    );
    expect(n).toBe(0);
    // The alarm re-arms itself.
    expect(
      await runInDurableObject(stub, (_i: Dashboard, state) =>
        state.storage.getAlarm(),
      ),
    ).not.toBeNull();
  });
});

describe('live updates', () => {
  it('the owner socket says hello, then changed after a write; a bad key is refused', async () => {
    const { owner } = await setup();
    const bad = await exports.default.fetch(
      new Request(`${O}/v1/owner/live`, {
        headers: { upgrade: 'websocket', authorization: 'Bearer nope' },
      }),
    );
    expect(bad.status).toBe(401);
    const r = await exports.default.fetch(
      new Request(`${O}/v1/owner/live`, {
        headers: { upgrade: 'websocket', authorization: `Bearer ${owner}` },
      }),
    );
    expect(r.status).toBe(101);
    const ws = r.webSocket!;
    const msgs: J[] = [];
    const got = (type: string) =>
      new Promise<void>((res) => {
        const tick = () =>
          msgs.some((m) => m.type === type) ? res() : setTimeout(tick, 10);
        tick();
      });
    ws.addEventListener('message', (e) => {
      msgs.push(JSON.parse(e.data as string));
    });
    ws.accept();
    await got('hello');
    await issue(owner);
    await got('changed');
    ws.close();
  });
});

describe('QRT-307: retention, unenroll, logging, content cap', () => {
  const count = (stub: DurableObjectStub<Dashboard>, table: string) =>
    runInDurableObject(
      stub,
      (_i: Dashboard, state) =>
        state.storage.sql
          .exec<{ n: number }>(`SELECT COUNT(*) AS n FROM ${table}`)
          .one().n,
    );
  const old = () => Date.now() - (C.RETENTION_DAYS + 1) * C.DAY_MS;

  it('prunes widgets (state_json) idle past 30 days and keeps fresh ones', async () => {
    const { dashId, owner } = await setup();
    const a = await approved(owner);
    for (const key of ['old', 'new']) {
      await call('POST', '/v1/widgets', {
        bearer: a.token,
        body: { key, type: 'log' },
      });
      await call('POST', `/v1/widgets/${key}/events`, {
        bearer: a.token,
        body: { line: `hi ${key}` },
      });
    }
    const stub = env.DASHBOARD.getByName(dashId);
    await runInDurableObject(stub, (_i: Dashboard, state) => {
      state.storage.sql.exec(
        "UPDATE widgets SET updated_at = ? WHERE key = 'old'",
        old(),
      );
    });
    expect(await runDurableObjectAlarm(stub)).toBe(true);
    const keys = await runInDurableObject(stub, (_i: Dashboard, state) =>
      state.storage.sql
        .exec<{ key: string }>('SELECT key FROM widgets')
        .toArray()
        .map((r) => r.key),
    );
    expect(keys).toEqual(['new']);
    expect(await count(stub, 'events')).toBe(1);
    // An expired widget is not tombstoned: the agent can register the key again.
    expect(
      (
        await call('POST', '/v1/widgets', {
          bearer: a.token,
          body: { key: 'old', type: 'log' },
        })
      ).status,
    ).toBe(201);
  });

  it('prunes agent rows (any status) quiet past 30 days with all their content; the token then answers 410', async () => {
    clearTombCache();
    const { dashId, owner } = await setup();
    const a = await approved(owner);
    const p = await enroll(owner, 'pending bot');
    const keep = await approved(owner, 'fresh bot');
    await call('POST', '/v1/widgets', {
      bearer: a.token,
      body: { key: 's', type: 'status' },
    });
    await call('PUT', '/v1/widgets/s', {
      bearer: a.token,
      body: { status: 'running', text: 'private' },
    });
    const stub = env.DASHBOARD.getByName(dashId);
    await runInDurableObject(stub, (_i: Dashboard, state) => {
      state.storage.sql.exec(
        'UPDATE agents SET last_seen = ? WHERE id IN (?, ?)',
        old(),
        a.id,
        p.id,
      );
    });
    expect(await runDurableObjectAlarm(stub)).toBe(true);
    const ids = await runInDurableObject(stub, (_i: Dashboard, state) =>
      state.storage.sql
        .exec<{ id: string }>('SELECT id FROM agents')
        .toArray()
        .map((r) => r.id),
    );
    expect(ids).toEqual([keep.id]);
    expect(await count(stub, 'widgets')).toBe(0);
    const r = await call('POST', '/v1/heartbeat', {
      bearer: a.token,
      body: { status: 'idle' },
    });
    expect(r.status).toBe(410);
    expect(r.json.scope).toBe('agent');
  });

  it('unenrolling an agent deletes its row, widgets, events and idempotency records', async () => {
    const { dashId, owner } = await setup();
    const a = await approved(owner);
    await call('POST', '/v1/widgets', {
      bearer: a.token,
      body: { key: 'c', type: 'chart' },
    });
    await call('POST', '/v1/widgets/c/events', {
      bearer: a.token,
      body: { value: 1 },
      headers: { 'idempotency-key': 'k1' },
    });
    const stub = env.DASHBOARD.getByName(dashId);
    expect(await count(stub, 'idempotency')).toBe(1);
    expect(
      (await call('DELETE', `/v1/owner/agents/${a.id}`, { bearer: owner })).json
        .ok,
    ).toBe(true);
    for (const t of ['agents', 'widgets', 'events', 'idempotency'])
      expect(await count(stub, t), t).toBe(0);
  });

  it('deleting the dashboard deletes agent rows too', async () => {
    const { dashId, owner } = await setup();
    await approved(owner);
    await call('DELETE', '/v1/owner/dashboard', { bearer: owner });
    expect(await count(env.DASHBOARD.getByName(dashId), 'agents')).toBe(0);
  });

  it('caps one stored state or event at MAX_CONTENT_BYTES, batch ops included', async () => {
    const { owner } = await setup();
    const a = await approved(owner);
    await call('POST', '/v1/widgets', {
      bearer: a.token,
      body: { key: 'l', type: 'log' },
    });
    const big = 'word '.repeat(2000);
    const r = await call('POST', '/v1/batch', {
      bearer: a.token,
      body: { ops: [{ op: 'append', key: 'l', data: { line: big } }] },
    });
    expect(r.status).toBe(200);
    expect(r.json.results[0].status).toBe(422);
  });

  it('the content cap edge matches the Rust twin: 8192 stored bytes pass, 8193 are 422 (batch and single put)', async () => {
    // Same builder as crates/qrtz-dash/tests/api.rs `table_of`: bytes, not characters.
    const tableOf = (n: number) => {
      const rows = Array.from({ length: 16 }, () => ['€'.repeat(160)]);
      rows.push(['€'.repeat(133) + (n > C.MAX_CONTENT_BYTES ? 'aaa' : 'aa')]);
      const v = { columns: ['c'], rows };
      expect(new TextEncoder().encode(JSON.stringify(v)).length).toBe(n);
      return v;
    };
    const { owner } = await setup();
    const a = await approved(owner);
    await call('POST', '/v1/widgets', {
      bearer: a.token,
      body: { key: 't', type: 'table' },
    });
    const batch = (data: unknown) =>
      call('POST', '/v1/batch', {
        bearer: a.token,
        body: { ops: [{ op: 'put', key: 't', data }] },
      }).then((r) => r.json.results[0].status);
    expect(await batch(tableOf(C.MAX_CONTENT_BYTES))).toBe(200);
    expect(await batch(tableOf(C.MAX_CONTENT_BYTES + 1))).toBe(422);
    expect(
      (
        await call('PUT', '/v1/widgets/t', {
          bearer: a.token,
          body: tableOf(C.MAX_CONTENT_BYTES),
        })
      ).status,
    ).toBe(200);
  });

  it('markers keep only scope, ref (id, token hmac or agent/key) and time; audit only event, agent id and time', async () => {
    clearTombCache();
    const { dashId, owner } = await setup();
    const a = await approved(owner);
    const b = await approved(owner, 'second helper');
    for (const ag of [a, b])
      await call('POST', '/v1/widgets', {
        bearer: ag.token,
        body: { key: 'w', type: 'status', title: 'secret title' },
      });
    await call('DELETE', `/v1/owner/widgets/${b.id}/w`, { bearer: owner });
    await call('DELETE', `/v1/owner/widgets/${a.id}/w`, { bearer: owner });
    await call('DELETE', `/v1/owner/agents/${a.id}`, { bearer: owner });
    const stub = env.DASHBOARD.getByName(dashId);
    const rows = await runInDurableObject(stub, (_i: Dashboard, state) => ({
      tombCols: state.storage.sql
        .exec("SELECT name FROM pragma_table_info('tombstones')")
        .toArray()
        .map((r) => r.name),
      tombs: state.storage.sql
        .exec<{ scope: string; ref: string }>(
          'SELECT scope, ref FROM tombstones',
        )
        .toArray()
        .map((r) => `${r.scope}|${r.ref}`)
        .sort(),
      auditCols: state.storage.sql
        .exec("SELECT name FROM pragma_table_info('audit')")
        .toArray()
        .map((r) => r.name),
    }));
    expect(rows.tombCols).toEqual(['scope', 'ref', 'created_at']);
    expect(rows.auditCols).toEqual(['id', 'event', 'agent_id', 'at']);
    // The removed agent's widget marker went with it; the live agent's stays. No names, titles or devices.
    expect(rows.tombs.filter((t) => t.startsWith('widget|'))).toEqual([
      `widget|${b.id}/w`,
    ]);
    expect(rows.tombs).toContain(`agent|${a.id}`);
    expect(rows.tombs.filter((t) => t.startsWith('agent|token:')).length).toBe(
      1,
    );
    expect(JSON.stringify(rows)).not.toMatch(/grok|secret title/i);
    // b's deleted widget still answers 410 after a fresh isolate reloads the markers.
    expect(
      (
        await call('PUT', '/v1/widgets/w', {
          bearer: b.token,
          body: { status: 'done' },
        })
      ).status,
    ).toBe(410);
    await call('DELETE', '/v1/owner/dashboard', { bearer: owner });
    const left = await runInDurableObject(stub, (_i: Dashboard, state) =>
      state.storage.sql
        .exec<{ scope: string }>(
          "SELECT scope FROM tombstones WHERE scope = 'widget'",
        )
        .toArray(),
    );
    expect(left).toEqual([]);
  });

  it('the prod config logs only the Worker line: no invocation logs, no traces, every request sampled', () => {
    const t = env.TEST_WRANGLER_TOML;
    expect(t).toMatch(
      /\[observability\]\nenabled = true\nhead_sampling_rate = 1\nredact_query_string = true\n\n\[observability\.logs\]\nenabled = true\nhead_sampling_rate = 1\ninvocation_logs = false\n\n\[observability\.traces\]\nenabled = false\n/,
    );
    expect(t.match(/^\[observability/gm)).toHaveLength(3);
    expect(t).not.toMatch(/^\[env\./m);
  });

  it('route templates never carry path text', () => {
    for (const [p, t] of [
      ['/v1/widgets/abc', '/v1/widgets/:key'],
      ['/v1/widgets/abc/events', '/v1/widgets/:key/events'],
      ['/v1/owner/agents/ag_1', '/v1/owner/agents/:id'],
      ['/v1/owner/agents/ag_1/review', '/v1/owner/agents/:id/review'],
      ['/v1/owner/widgets/ag_1/k', '/v1/owner/widgets/:agent/:key'],
      ['/v1/batch', '/v1/batch'],
      ['/v1/widgets//events', 'unmatched'],
      ['/v1/secret/stuff', 'unmatched'],
      ['/at_0123', 'unmatched'],
      ['', 'unmatched'],
    ])
      expect(routeTemplate(p!), p).toBe(t);
  });

  it('logs one line per request: method, route template, status, ms, and nothing from the request', async () => {
    const lines: string[] = [];
    const secrets: string[] = [];
    const orig = console.log;
    console.log = (...x: unknown[]) => void lines.push(x.map(String).join(' '));
    try {
      const { owner } = await setup();
      const a = await approved(owner);
      secrets.push(a.id, a.token, owner);
      lines.length = 0;
      await call('POST', '/v1/widgets', {
        bearer: a.token,
        body: { key: 'my-private-key', type: 'status', title: 'body text' },
      });
      await exports.default.fetch(
        new Request('https://dash.qrtz.ai/v1/widgets/my-private-key?q=leak', {
          method: 'PUT',
          headers: {
            authorization: `Bearer ${a.token}`,
            'user-agent': 'ua-leak',
            'cf-connecting-ip': '192.0.2.99',
          },
          body: JSON.stringify({ status: 'done', text: 'body text' }),
        }),
      );
      await call('DELETE', `/v1/owner/agents/${a.id}`, { bearer: owner });
      await call('GET', '/nope/../x');
    } finally {
      console.log = orig;
    }
    const parsed = lines.map((l) => JSON.parse(l));
    for (const p of parsed) {
      expect(Object.keys(p).sort()).toEqual([
        'method',
        'ms',
        'route',
        'status',
      ]);
      expect(typeof p.ms).toBe('number');
    }
    expect(parsed.map((p) => `${p.method} ${p.route} ${p.status}`)).toEqual([
      'POST /v1/widgets 201',
      'PUT /v1/widgets/:key 200',
      'DELETE /v1/owner/agents/:id 200',
      'GET unmatched 404',
    ]);
    const all = lines.join('\n');
    for (const leak of [
      'my-private-key',
      'leak',
      'body text',
      ...secrets,
      '192.0.2.99',
      'dash.qrtz.ai',
    ])
      expect(all).not.toContain(leak);
  });
});

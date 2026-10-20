/**
 * Agent Dash server at https://dash.qrtz.ai (ADR 0228 amendment). The Worker routes, caps bodies from
 * Content-Length and the stream, checks bearer shape, HMACs the bearer with the pepper and checks the cached
 * tombstones before any Durable Object is touched; the dashboard DO checks auth before it parses any JSON
 * (except /v1/enroll, which has no bearer). Bearers are only ever read from the Authorization header.
 */
import * as C from './constants';
import { ctEq, hmacHex, pepperOk, randomHex } from './crypto';
import {
  fail,
  gone,
  out,
  SECURITY_HEADERS,
  stop,
  toResponse,
  type Out,
  type Scope,
} from './http';
import { README_MD } from './readme.gen';
import type { Env } from './env';

export { Dashboard } from './dashboard';
import { unwire } from './dashboard';
export { EnrollDirectory } from './directory';
import { sourceOf } from './ip';
import { devLocal, TEST_CODE_PREFIX } from './dev';

const AGENT_TOKEN = /^at_([0-9a-f]{32})_[0-9a-f]{64}$/;
const OWNER_KEY = /^ok_([0-9a-f]{32})_[0-9a-f]{64}$/;

/** Isolate cache of tombstoned bearers (HMAC → scope) and deleted dashboards, filled from DO answers. */
const TOMB_CACHE_MAX = 10_000;
const tombCache = new Map<string, Scope>();
const remember = (k: string, scope: Scope) => {
  if (tombCache.size >= TOMB_CACHE_MAX)
    tombCache.delete(tombCache.keys().next().value as string);
  tombCache.set(k, scope);
};
/** Test seam. */
export const clearTombCache = () => tombCache.clear();

const bearer = (req: Request) => {
  const h = req.headers.get('authorization');
  if (!h || !h.startsWith('Bearer ')) return null;
  return h.slice(7).trim();
};

/** The body as text, refused past `max` bytes from Content-Length or the stream itself. */
const readCapped = async (req: Request, max: number): Promise<string | Out> => {
  const declared = req.headers.get('content-length');
  if (
    declared !== null &&
    (!/^\d+$/.test(declared.trim()) || Number(declared) > max)
  )
    return fail(413, 'body too large');
  if (!req.body) return '';
  const reader = req.body.getReader();
  const chunks: Uint8Array[] = [];
  let n = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    n += value.byteLength;
    if (n > max) {
      await reader.cancel();
      return fail(413, 'body too large');
    }
    chunks.push(value);
  }
  const all = new Uint8Array(n);
  let o = 0;
  for (const c of chunks) {
    all.set(c, o);
    o += c.byteLength;
  }
  try {
    return new TextDecoder('utf-8', { fatal: true, ignoreBOM: false }).decode(
      all,
    );
  } catch {
    return fail(422, 'body is not utf-8');
  }
};

async function enroll(req: Request, env: Env): Promise<Out> {
  const text = await readCapped(req, C.MAX_BODY_BYTES);
  if (typeof text !== 'string') return text;
  let body: Record<string, unknown>;
  try {
    const v = JSON.parse(text);
    body = v && typeof v === 'object' ? v : {};
  } catch {
    return fail(422, 'body is not json');
  }
  // Bozniak M2: no address, no attempt (never one shared 'unknown' bucket). Cloudflare always sets this header.
  const dev = devLocal(env, req);
  const source = sourceOf(req.headers.get('cf-connecting-ip'));
  if (!source) return fail(400, 'client address missing');
  const src = {
    narrow: await hmacHex(env.DASH_PEPPER, `ip:${source.narrow}`),
    wide:
      source.wide === null
        ? null
        : await hmacHex(env.DASH_PEPPER, `ip:${source.wide}`),
  };
  const code = (typeof body.code === 'string' ? body.code : '')
    .trim()
    .toUpperCase();
  const codeHash = await hmacHex(env.DASH_PEPPER, code);
  const dir = env.ENROLL.getByName('enroll');
  // A test code outside local test mode: the same answer as a wrong code, and counted as one.
  if (code.startsWith(TEST_CODE_PREFIX) && !dev) {
    await dir.fail(src);
    return fail(401, 'enrollment code is used, expired or wrong');
  }
  const claim = await dir.claim(src, codeHash);
  if (!claim.ok) {
    if (claim.status === 429)
      return out(429, {
        error: 'too many wrong codes',
        retry_after_ms: claim.retryAfterMs ?? 60_000,
      });
    return fail(401, 'enrollment code is used, expired or wrong');
  }
  const token = `at_${claim.dashId}_${randomHex(32)}`;
  const tokenHash = await hmacHex(env.DASH_PEPPER, token);
  const r = unwire(
    await env.DASHBOARD.getByName(claim.dashId).enroll({
      codeHash,
      name: body.name,
      description: body.description,
      tokenHash,
    }),
  );
  if (r.status === 401) await dir.fail(src);
  // The token is only ever in this one response.
  return r.status === 201
    ? { status: 201, body: { ...r.body, agent_token: token } }
    : r;
}

async function agent(req: Request, env: Env, path: string): Promise<Out> {
  const token = bearer(req);
  const m = token ? AGENT_TOKEN.exec(token) : null;
  if (!token || !m) return stop(401, 'missing or invalid token');
  const dashId = m[1]!;
  if (tombCache.get(`dash:${dashId}`)) return gone('dashboard');
  const tokenHash = await hmacHex(env.DASH_PEPPER, token);
  const cached = tombCache.get(`tok:${tokenHash}`);
  if (cached) return gone(cached);
  const text = await readCapped(
    req,
    path === '/v1/batch' ? C.MAX_BATCH_BODY_BYTES : C.MAX_BODY_BYTES,
  );
  if (typeof text !== 'string') return text;
  const r = unwire(
    await env.DASHBOARD.getByName(dashId).agent({
      tokenHash,
      method: req.method,
      path,
      body: text,
      idempotencyKey: req.headers.get('idempotency-key'),
    }),
  );
  if (r.status === 410 && r.body.scope === 'dashboard')
    remember(`dash:${dashId}`, 'dashboard');
  if (r.status === 410 && r.body.scope === 'agent')
    remember(`tok:${tokenHash}`, 'agent');
  return r;
}

async function setup(req: Request, env: Env): Promise<Out> {
  const t = bearer(req) ?? '';
  // Compare HMACs, so the compare is constant-time and length-independent.
  const ok =
    typeof env.DASH_SETUP_TOKEN === 'string' &&
    env.DASH_SETUP_TOKEN.length >= 32 &&
    ctEq(
      await hmacHex(env.DASH_PEPPER, `setup:${t}`),
      await hmacHex(env.DASH_PEPPER, `setup:${env.DASH_SETUP_TOKEN}`),
    );
  if (!ok) return stop(401, 'setup token invalid');
  // Bozniak M1: one dashboard per setup token, ever; the ADR deploy steps rotate the secret afterwards too.
  if (!(await env.ENROLL.getByName('enroll').useSetup()))
    return fail(410, 'setup already used');
  const dashId = randomHex(16);
  const ownerKey = `ok_${dashId}_${randomHex(32)}`;
  const created = await env.DASHBOARD.getByName(dashId).setup(
    dashId,
    await hmacHex(env.DASH_PEPPER, ownerKey),
  );
  if (!created) return fail(409, 'dashboard exists');
  // Shown once; the app keeps it in the macOS Keychain only.
  return out(201, { dashboard_id: dashId, owner_key: ownerKey });
}

async function owner(
  req: Request,
  env: Env,
  path: string,
): Promise<Out | Response> {
  const key = bearer(req);
  const m = key ? OWNER_KEY.exec(key) : null;
  if (!key || !m) return stop(401, 'owner key invalid');
  const ownerHash = await hmacHex(env.DASH_PEPPER, key);
  const stub = env.DASHBOARD.getByName(m[1]!);
  if (path === '/v1/owner/live') {
    const h = new Headers({
      upgrade: req.headers.get('upgrade') ?? '',
      'x-dash-owner-hash': ownerHash,
    });
    return stub.fetch(new Request('https://do/live', { headers: h }));
  }
  const text = await readCapped(req, C.MAX_BODY_BYTES);
  if (typeof text !== 'string') return text;
  return unwire(
    await stub.owner({ ownerHash, method: req.method, path, body: text }),
  );
}

async function route(req: Request, env: Env): Promise<Out | Response> {
  const url = new URL(req.url);
  if (
    url.protocol !== 'https:' &&
    !(url.protocol === 'http:' && devLocal(env, req))
  )
    return fail(400, 'https only');
  if (!pepperOk(env.DASH_PEPPER)) return fail(503, 'server not configured');
  // Nothing is selected by query string, and tokens never go in URLs.
  const path = url.pathname;
  if (req.method === 'GET' && path === '/v1/readme')
    return new Response(README_MD, {
      headers: {
        ...SECURITY_HEADERS,
        'content-type': 'text/markdown; charset=utf-8',
        'x-readme-version': C.README_VERSION,
      },
    });
  if (req.method === 'POST' && path === '/v1/enroll') return enroll(req, env);
  if (req.method === 'POST' && path === '/v1/owner/setup')
    return setup(req, env);
  if (path.startsWith('/v1/owner/')) return owner(req, env, path);
  if (path.startsWith('/v1/')) return agent(req, env, path);
  return fail(404, 'no such endpoint');
}

export default {
  async fetch(req, env): Promise<Response> {
    try {
      const r = await route(req, env);
      return r instanceof Response ? r : toResponse(r);
    } catch {
      // Never echo internals.
      return toResponse(fail(500, 'server error'));
    }
  },
} satisfies ExportedHandler<Env>;

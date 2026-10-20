/**
 * One Durable Object per dashboard (spec "Stack"): serialized writes, SQLite storage, rate limits, tombstones,
 * the owner's read-only view and live updates over hibernatable WebSockets, and the retention alarm.
 * The rules are crates/qrtz-dash/src/store.rs `Dash`; the Worker (index.ts) only routes, caps bodies and hashes
 * bearers, so raw agent tokens and owner keys never reach this object, only their HMACs.
 */
import { DurableObject } from 'cloudflare:workers';
import {
  parseBlockType,
  validateAppend,
  validatePut,
  type BlockType,
} from './blocks';
import * as C from './constants';
import { codeMac, ctEq, newCode, pepperOk, randomHex } from './crypto';
import { devCodes, TEST_CODE_PREFIX } from './dev';
import {
  fail,
  gone,
  out,
  stop,
  type JsonObj,
  type Out,
  type Scope,
} from './http';
import { charCount, cleanName, RESERVED, skeleton, validKey } from './names';
import { scrub, scrubValue } from './scrub';
import type { Env } from './env';

type AgentStatus = 'pending' | 'approved' | 'rejected';
type AgentRow = {
  id: string;
  name: string;
  skeleton: string;
  description: string;
  fingerprint: string;
  token_hash: string;
  status: AgentStatus;
  heartbeat: string | null;
  last_seen: number;
  created_at: number;
  flagged: number;
};
type WidgetRow = {
  id: string;
  agent_id: string;
  key: string;
  type: BlockType;
  title: string;
  state_json: string | null;
  updated_at: number;
};

export interface AgentCall {
  tokenHash: string;
  method: string;
  path: string;
  body: string;
  idempotencyKey: string | null;
  now?: number;
}
export interface OwnerCall {
  ownerHash: string;
  method: string;
  path: string;
  body: string;
  now?: number;
  /** Set by the Worker only for POST /v1/owner/test-codes in local test mode (src/dev.ts). */
  testCodes?: boolean;
}

export type Wire = { status: number; json: string };
const wire = (o: Out): Wire => ({
  status: o.status,
  json: JSON.stringify(o.body),
});
export const unwire = (w: Wire): Out => ({
  status: w.status,
  body: JSON.parse(w.json) as JsonObj,
});

const SCHEMA = `
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
DROP TABLE IF EXISTS enroll_codes;
CREATE TABLE IF NOT EXISTS enroll_code_macs (code_id TEXT PRIMARY KEY, secret_mac TEXT NOT NULL,
  expires_at INTEGER NOT NULL, used_at INTEGER);
CREATE TABLE IF NOT EXISTS agents (id TEXT PRIMARY KEY, name TEXT NOT NULL, skeleton TEXT NOT NULL,
  description TEXT NOT NULL, fingerprint TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE, status TEXT NOT NULL,
  heartbeat TEXT, last_seen INTEGER NOT NULL, created_at INTEGER NOT NULL, flagged INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS widgets (id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, key TEXT NOT NULL,
  type TEXT NOT NULL, title TEXT NOT NULL, state_json TEXT, updated_at INTEGER NOT NULL, UNIQUE (agent_id, key));
CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY AUTOINCREMENT, widget_id TEXT NOT NULL,
  at INTEGER NOT NULL, data_json TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS events_widget ON events (widget_id, id);
CREATE INDEX IF NOT EXISTS events_at ON events (at);
CREATE TABLE IF NOT EXISTS idempotency (agent_id TEXT NOT NULL, k TEXT NOT NULL, expires_at INTEGER NOT NULL,
  status INTEGER NOT NULL, body_json TEXT NOT NULL, PRIMARY KEY (agent_id, k));
CREATE TABLE IF NOT EXISTS tombstones (scope TEXT NOT NULL, ref TEXT NOT NULL, created_at INTEGER NOT NULL,
  PRIMARY KEY (scope, ref));
CREATE TABLE IF NOT EXISTS audit (id INTEGER PRIMARY KEY AUTOINCREMENT, event TEXT NOT NULL, agent_id TEXT,
  at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS enroll_failures (at INTEGER NOT NULL);
`;

export class Dashboard extends DurableObject<Env> {
  private sql: SqlStorage;
  /** In-memory twin of the tombstones table (spec: checked before any storage hit). */
  private tombs = new Set<string>();
  private agentWrites = new Map<string, number[]>();
  private dashWrites: number[] = [];
  private lastPush = 0;
  private pushTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    ctx.blockConcurrencyWhile(async () => {
      this.sql.exec(SCHEMA);
      for (const t of this.sql.exec<{ scope: string; ref: string }>(
        'SELECT scope, ref FROM tombstones',
      ))
        this.tombs.add(`${t.scope}|${t.ref}`);
    });
    ctx.setWebSocketAutoResponse(
      new WebSocketRequestResponsePair('ping', 'pong'),
    );
  }

  // ---- helpers ----
  private meta(k: string) {
    return (
      this.sql
        .exec<{ v: string }>('SELECT v FROM meta WHERE k = ?', k)
        .toArray()[0]?.v ?? null
    );
  }
  private setMeta(k: string, v: string) {
    this.sql.exec(
      'INSERT INTO meta (k, v) VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v',
      k,
      v,
    );
  }
  private get deleted() {
    return this.meta('deleted') === '1';
  }
  private tomb(scope: Scope, ref: string, now: number) {
    this.sql.exec(
      'INSERT OR IGNORE INTO tombstones (scope, ref, created_at) VALUES (?, ?, ?)',
      scope,
      ref,
      now,
    );
    this.tombs.add(`${scope}|${ref}`);
  }
  private log(event: string, agentId: string | null, now: number) {
    this.sql.exec(
      'INSERT INTO audit (event, agent_id, at) VALUES (?, ?, ?)',
      event,
      agentId,
      now,
    );
  }
  private agents() {
    return this.sql
      .exec<AgentRow>('SELECT * FROM agents ORDER BY created_at, id')
      .toArray();
  }
  private widget(agentId: string, key: string) {
    return this.sql
      .exec<WidgetRow>(
        'SELECT * FROM widgets WHERE agent_id = ? AND key = ?',
        agentId,
        key,
      )
      .toArray()[0];
  }
  private async ensureAlarm(now: number) {
    if ((await this.ctx.storage.getAlarm()) === null)
      await this.ctx.storage.setAlarm(now + C.PRUNE_EVERY_MS);
  }
  /** Tell every owner socket the view changed (at most one push per LIVE_THROTTLE_MS; the app refetches). */
  private changed() {
    const send = () => {
      this.pushTimer = null;
      this.lastPush = Date.now();
      const msg = JSON.stringify({ type: 'changed', at: this.lastPush });
      for (const ws of this.ctx.getWebSockets('owner')) {
        try {
          ws.send(msg);
        } catch {
          /* closed */
        }
      }
    };
    if (this.pushTimer) return;
    const wait = this.lastPush + C.LIVE_THROTTLE_MS - Date.now();
    if (wait <= 0) send();
    else this.pushTimer = setTimeout(send, wait);
  }

  // ---- RPC (answers cross as a JSON string: an Out's body is arbitrary JSON) ----
  async enroll(a: {
    codeId: string;
    secretMac: string;
    name: unknown;
    description: unknown;
    tokenHash: string;
    now?: number;
  }): Promise<Wire> {
    return wire(await this.enrollImpl(a));
  }
  async agent(c: AgentCall): Promise<Wire> {
    return wire(await this.agentImpl(c));
  }
  async owner(c: OwnerCall): Promise<Wire> {
    return wire(await this.ownerImpl(c));
  }

  // ---- setup (Worker, after it checked the setup token) ----
  /** Store the owner key's HMAC. Only once: a dashboard has one owner key. */
  async setup(
    dashId: string,
    ownerHash: string,
    now = Date.now(),
  ): Promise<boolean> {
    if (this.meta('owner_hash') !== null) return false;
    this.setMeta('dash_id', dashId);
    this.setMeta('owner_hash', ownerHash);
    this.setMeta('created_at', String(now));
    this.log('dashboard-created', null, now);
    await this.ensureAlarm(now);
    return true;
  }

  // ---- the agent's side ----
  private async enrollImpl(a: {
    codeId: string;
    secretMac: string;
    name: unknown;
    description: unknown;
    tokenHash: string;
    now?: number;
  }): Promise<Out> {
    const now = a.now ?? Date.now();
    if (this.deleted) return gone('dashboard');
    const win = now - 60_000;
    this.sql.exec('DELETE FROM enroll_failures WHERE at <= ?', win);
    const recent = this.sql
      .exec<{ n: number; oldest: number | null }>(
        'SELECT COUNT(*) AS n, MIN(at) AS oldest FROM enroll_failures',
      )
      .one();
    if (recent.n >= C.ENROLL_FAILS_PER_MIN)
      return out(429, {
        error: 'too many wrong codes',
        retry_after_ms: Math.max(1000, (recent.oldest ?? now) + 60_000 - now),
      });
    // QRT-284: found by id, then the secret's HMAC compared in constant time (a dummy compare when no row).
    const code = this.sql
      .exec<{ code_id: string; secret_mac: string }>(
        'SELECT code_id, secret_mac FROM enroll_code_macs WHERE code_id = ? AND used_at IS NULL AND expires_at > ?',
        a.codeId,
        now,
      )
      .toArray()[0];
    const match = ctEq(code?.secret_mac ?? '0'.repeat(64), a.secretMac);
    if (!code || !match) {
      this.sql.exec('INSERT INTO enroll_failures (at) VALUES (?)', now);
      const total = Number(this.meta('enroll_fail_total') ?? '0') + 1;
      this.setMeta('enroll_fail_total', String(total));
      if (total >= C.ENROLL_FAILS_BEFORE_ROTATION) {
        this.rotate(now);
        this.log('codes-rotated-after-failures', null, now);
      }
      return fail(401, 'enrollment code is used, expired or wrong');
    }
    // Burned before anything else is checked.
    this.sql.exec(
      'UPDATE enroll_code_macs SET used_at = ? WHERE code_id = ?',
      now,
      code.code_id,
    );
    const name = cleanName(typeof a.name === 'string' ? a.name : '');
    let description = cleanName(
      typeof a.description === 'string' ? a.description : '',
    );
    if (!name || charCount(name) > 40 || charCount(description) > 200)
      return fail(422, 'name is 1-40 characters, description at most 200');
    const sk = skeleton(name);
    if (!sk || RESERVED.some((r) => skeleton(r) === sk))
      return fail(422, 'that name is reserved');
    const all = this.agents();
    if (all.some((x) => x.skeleton === sk && x.status !== 'rejected'))
      return fail(409, 'an agent with that name already exists');
    if (all.length >= C.MAX_AGENTS)
      return fail(422, 'this dashboard has the most agents it can hold');
    const s = scrub(description);
    description = s.text;
    const f = a.tokenHash.slice(0, 8).toUpperCase();
    const fingerprint = `${f.slice(0, 4)}-${f.slice(4)}`;
    const id = `ag_${randomHex(8)}`;
    this.sql.exec(
      `INSERT INTO agents (id, name, skeleton, description, fingerprint, token_hash, status, heartbeat, last_seen,
        created_at, flagged) VALUES (?, ?, ?, ?, ?, ?, 'pending', NULL, ?, ?, ?)`,
      id,
      name,
      sk,
      description,
      fingerprint,
      a.tokenHash,
      now,
      now,
      s.hit ? 1 : 0,
    );
    this.log('enrolled', id, now);
    this.changed();
    return out(201, { agent_id: id, status: 'pending', fingerprint });
  }

  /** Every agent write. Auth (by token HMAC) happens before the body is parsed. */
  private async agentImpl(c: AgentCall): Promise<Out> {
    const now = c.now ?? Date.now();
    if (this.deleted) return gone('dashboard');
    const ag = this.sql
      .exec<AgentRow>('SELECT * FROM agents WHERE token_hash = ?', c.tokenHash)
      .toArray()[0];
    if (!ag || !ctEq(ag.token_hash, c.tokenHash)) {
      if (this.tombs.has(`agent|token:${c.tokenHash}`)) return gone('agent');
      return stop(401, 'token invalid or revoked');
    }
    if (ag.status === 'rejected') return stop(401, 'token invalid or revoked');
    if (ag.status === 'pending') return out(403, { error: 'pending' });
    let body: unknown = null;
    if (c.body.length > 0) {
      try {
        body = JSON.parse(c.body);
      } catch {
        return fail(422, 'body is not json');
      }
    }
    const b = (body ?? {}) as Record<string, unknown>;
    const seq = typeof b === 'object' && b !== null ? b.seq : undefined;
    const idem =
      c.idempotencyKey ??
      (typeof seq === 'number' && Number.isInteger(seq) && seq >= 0
        ? `seq:${seq}`
        : null);
    if (idem) {
      const hit = this.sql
        .exec<{ status: number; body_json: string }>(
          'SELECT status, body_json FROM idempotency WHERE agent_id = ? AND k = ? AND expires_at > ?',
          ag.id,
          idem,
          now,
        )
        .toArray()[0];
      if (hit)
        return {
          status: hit.status,
          body: JSON.parse(hit.body_json) as JsonObj,
        };
    }
    const ops =
      c.path === '/v1/batch'
        ? Math.max(
            1,
            Array.isArray((b as { ops?: unknown }).ops)
              ? (b.ops as unknown[]).length
              : 1,
          )
        : 1;
    const limited = this.rate(ag.id, ops, now);
    if (limited) return limited;
    this.sql.exec('UPDATE agents SET last_seen = ? WHERE id = ?', now, ag.id);
    const r = this.route(ag.id, c.method, c.path, body, now);
    if (idem && r.status < 300)
      this.sql.exec(
        'INSERT OR REPLACE INTO idempotency (agent_id, k, expires_at, status, body_json) VALUES (?, ?, ?, ?, ?)',
        ag.id,
        idem,
        now + C.IDEMPOTENCY_TTL_MS,
        r.status,
        JSON.stringify(r.body),
      );
    await this.ensureAlarm(now);
    this.changed();
    return r;
  }

  private rate(agentId: string, n: number, now: number): Out | null {
    const win = now - 60_000;
    const a = (this.agentWrites.get(agentId) ?? []).filter((t) => t > win);
    this.dashWrites = this.dashWrites.filter((t) => t > win);
    if (
      a.length + n > C.WRITES_PER_MIN_AGENT ||
      this.dashWrites.length + n > C.WRITES_PER_MIN_DASHBOARD
    ) {
      const oldest = Math.min(a[0] ?? now, this.dashWrites[0] ?? now);
      this.agentWrites.set(agentId, a);
      return out(429, {
        error: 'rate limited',
        retry_after_ms: Math.max(1000, oldest + 60_000 - now),
      });
    }
    for (let i = 0; i < n; i++) {
      a.push(now);
      this.dashWrites.push(now);
    }
    this.agentWrites.set(agentId, a);
    return null;
  }

  private route(
    agent: string,
    method: string,
    path: string,
    body: unknown,
    now: number,
  ): Out {
    const parts = path.replace(/^\/+/, '').split('/');
    const b = (body && typeof body === 'object' ? body : {}) as Record<
      string,
      unknown
    >;
    if (method === 'POST' && path === '/v1/heartbeat') {
      if (b.status !== 'running' && b.status !== 'idle')
        return fail(422, 'status is running or idle');
      this.sql.exec(
        'UPDATE agents SET heartbeat = ? WHERE id = ?',
        b.status,
        agent,
      );
      return out(200, { ok: true });
    }
    if (method === 'POST' && path === '/v1/widgets')
      return this.register(agent, b, now);
    if (
      parts[0] === 'v1' &&
      parts[1] === 'widgets' &&
      parts.length === 3 &&
      method === 'PUT'
    )
      return this.write(agent, parts[2]!, false, body, now);
    if (
      parts[0] === 'v1' &&
      parts[1] === 'widgets' &&
      parts.length === 4 &&
      parts[3] === 'events' &&
      method === 'POST'
    )
      return this.write(agent, parts[2]!, true, body, now);
    if (method === 'POST' && path === '/v1/batch') {
      if (!Array.isArray(b.ops)) return fail(422, 'ops is a list');
      const ops = b.ops as Record<string, unknown>[];
      if (ops.length === 0 || ops.length > C.BATCH_MAX_OPS)
        return fail(422, `a batch has 1 to ${C.BATCH_MAX_OPS} ops`);
      const results = ops.map((op) => {
        const o = (op && typeof op === 'object' ? op : {}) as Record<
          string,
          unknown
        >;
        const key = typeof o.key === 'string' ? o.key : '';
        const r =
          o.op === 'put'
            ? this.write(agent, key, false, o.data ?? null, now)
            : o.op === 'append'
              ? this.write(agent, key, true, o.data ?? null, now)
              : fail(422, 'op is put or append');
        const { readme_version: _, ...result } = r.body;
        return { key, status: r.status, result };
      });
      return out(200, { results });
    }
    return fail(404, 'no such endpoint');
  }

  private register(
    agent: string,
    b: Record<string, unknown>,
    now: number,
  ): Out {
    const key = typeof b.key === 'string' ? b.key : '';
    if (!validKey(key)) return fail(422, 'key is lowercase kebab-case');
    if (this.tombs.has(`widget|${agent}/${key}`)) return gone('widget');
    const ty = parseBlockType(b.type);
    if (!ty) return fail(422, 'unknown type');
    let title = cleanName(typeof b.title === 'string' ? b.title : key);
    if (charCount(title) > 80)
      return fail(422, 'title is at most 80 characters');
    const s = scrub(title);
    title = s.text;
    const w = this.widget(agent, key);
    if (w) {
      if (w.type !== ty) return fail(409, 'key exists with another type');
      return out(201, { key, widget_id: w.id });
    }
    const n = this.sql
      .exec<{ n: number }>(
        'SELECT COUNT(*) AS n FROM widgets WHERE agent_id = ?',
        agent,
      )
      .one().n;
    if (n >= C.MAX_WIDGETS_PER_AGENT)
      return fail(422, 'at most 20 widgets per agent');
    const id = `w_${randomHex(8)}`;
    this.sql.exec(
      'INSERT INTO widgets (id, agent_id, key, type, title, state_json, updated_at) VALUES (?, ?, ?, ?, ?, NULL, ?)',
      id,
      agent,
      key,
      ty,
      title,
      now,
    );
    if (s.hit)
      this.sql.exec('UPDATE agents SET flagged = 1 WHERE id = ?', agent);
    return out(201, { key, widget_id: id });
  }

  private write(
    agent: string,
    key: string,
    append: boolean,
    data: unknown,
    now: number,
  ): Out {
    if (this.tombs.has(`widget|${agent}/${key}`)) return gone('widget');
    const w = this.widget(agent, key);
    if (!w)
      return out(404, {
        error: 'no such widget',
        action: 'register_own_widget',
      });
    if (data && typeof data === 'object' && !Array.isArray(data)) {
      const { seq: _, ...rest } = data as Record<string, unknown>;
      data = rest;
    }
    const err = append
      ? validateAppend(w.type, data)
      : validatePut(w.type, data);
    if (err) return fail(422, err);
    const s = scrubValue(data);
    const json = JSON.stringify(s.value);
    if (append) {
      this.sql.exec(
        'INSERT INTO events (widget_id, at, data_json) VALUES (?, ?, ?)',
        w.id,
        now,
        json,
      );
      const cap = w.type === 'log' ? C.LOG_RING : C.CHART_RING;
      this.sql.exec(
        `DELETE FROM events WHERE widget_id = ? AND id NOT IN
          (SELECT id FROM events WHERE widget_id = ? ORDER BY id DESC LIMIT ?)`,
        w.id,
        w.id,
        cap,
      );
      this.sql.exec(
        'UPDATE widgets SET updated_at = ? WHERE id = ?',
        now,
        w.id,
      );
    } else {
      this.sql.exec(
        'UPDATE widgets SET state_json = ?, updated_at = ? WHERE id = ?',
        json,
        now,
        w.id,
      );
    }
    if (s.hit)
      this.sql.exec('UPDATE agents SET flagged = 1 WHERE id = ?', agent);
    return out(200, { ok: true });
  }

  // ---- the owner's side ----
  private ownerOk(hash: string) {
    const h = this.meta('owner_hash');
    return h !== null && ctEq(h, hash);
  }

  /** Owner routes. The owner key's HMAC is checked before the body is parsed. */
  private async ownerImpl(c: OwnerCall): Promise<Out> {
    const now = c.now ?? Date.now();
    if (!this.ownerOk(c.ownerHash)) return stop(401, 'owner key invalid');
    let b: Record<string, unknown> = {};
    if (c.body.length > 0) {
      try {
        const v = JSON.parse(c.body);
        if (v && typeof v === 'object') b = v;
      } catch {
        return fail(422, 'body is not json');
      }
    }
    let p: string[]; // ['v1','owner',...]
    try {
      p = c.path.split('/').filter(Boolean).map(decodeURIComponent);
    } catch {
      return fail(404, 'no such endpoint'); // a malformed %-escape (Bozniak L5), not a 500
    }
    const m = c.method;
    const r = await (async (): Promise<Out> => {
      if (m === 'GET' && p[2] === 'view' && p.length === 3)
        return out(200, this.view(now));
      if (m === 'POST' && p[2] === 'codes' && p.length === 3)
        return await this.issueCode(now);
      // Local test mode only: the Worker sets testCodes after devLocal(); anything else is 'no such endpoint'.
      if (m === 'POST' && p[2] === 'test-codes' && p.length === 3)
        return c.testCodes === true && devCodes(this.env)
          ? await this.issueCode(now, TEST_CODE_PREFIX)
          : fail(404, 'no such endpoint');
      if (
        m === 'POST' &&
        p[2] === 'codes' &&
        p[3] === 'rotate' &&
        p.length === 4
      ) {
        this.rotate(now);
        return out(200, { ok: true });
      }
      if (
        m === 'POST' &&
        p[2] === 'agents' &&
        p[4] === 'review' &&
        p.length === 5
      )
        return out(200, { ok: this.review(p[3]!, b.approve === true, now) });
      if (m === 'DELETE' && p[2] === 'agents' && p.length === 4)
        return out(200, { ok: this.deleteAgent(p[3]!, now) });
      if (m === 'DELETE' && p[2] === 'widgets' && p.length === 5)
        return out(200, { ok: this.deleteWidget(p[3]!, p[4]!, now) });
      if (m === 'DELETE' && p[2] === 'dashboard' && p.length === 3) {
        this.deleteDashboard(now);
        return out(200, { ok: true });
      }
      return fail(404, 'no such endpoint');
    })();
    // A mutating call with a 2xx changes the view.
    if (m !== 'GET' && r.status < 300) {
      await this.ensureAlarm(now);
      this.changed();
    }
    return r;
  }

  /**
   * A single-use `<id>.<secret>` code, 15 minutes (QRT-284). Only HMAC(DASH_ENROLL_KEY, id.secret) is stored, here and
   * in the enroll directory; the secret itself is only in this one response.
   */
  private async issueCode(now: number, prefix = 'ENR-'): Promise<Out> {
    if (this.deleted) return fail(422, 'this dashboard was deleted');
    if (!pepperOk(this.env.DASH_ENROLL_KEY))
      return fail(503, 'server not configured');
    const { code, id, secret } = newCode(prefix);
    const mac = await codeMac(this.env.DASH_ENROLL_KEY, id, secret);
    const expiresAt = now + C.ENROLL_TTL_MS;
    this.sql.exec(
      'INSERT INTO enroll_code_macs (code_id, secret_mac, expires_at, used_at) VALUES (?, ?, ?, NULL)',
      id,
      mac,
      expiresAt,
    );
    this.log('code-issued', null, now);
    await this.env.ENROLL.getByName('enroll').register(
      id,
      mac,
      this.meta('dash_id') ?? '',
      expiresAt,
    );
    return out(201, { code, expiresAt });
  }

  private rotate(now: number) {
    this.sql.exec('DELETE FROM enroll_code_macs');
    this.sql.exec('DELETE FROM enroll_failures');
    this.setMeta('enroll_fail_total', '0');
    this.log('codes-rotated', null, now);
  }

  private review(agentId: string, approve: boolean, now: number) {
    const a = this.sql
      .exec<AgentRow>('SELECT * FROM agents WHERE id = ?', agentId)
      .toArray()[0];
    if (!a || a.status !== 'pending') return false;
    this.sql.exec(
      'UPDATE agents SET status = ? WHERE id = ?',
      approve ? 'approved' : 'rejected',
      agentId,
    );
    this.log(approve ? 'approved' : 'rejected', agentId, now);
    return true;
  }

  private deleteWidget(agentId: string, key: string, now: number) {
    const w = this.widget(agentId, key);
    if (!w) return false;
    this.ctx.storage.transactionSync(() => {
      this.sql.exec('DELETE FROM events WHERE widget_id = ?', w.id);
      this.sql.exec('DELETE FROM widgets WHERE id = ?', w.id);
      this.tomb('widget', `${agentId}/${key}`, now);
      this.log('widget-deleted', agentId, now);
    });
    return true;
  }

  private deleteAgent(agentId: string, now: number) {
    const a = this.sql
      .exec<AgentRow>('SELECT * FROM agents WHERE id = ?', agentId)
      .toArray()[0];
    if (!a) return false;
    this.ctx.storage.transactionSync(() => {
      this.sql.exec(
        'DELETE FROM events WHERE widget_id IN (SELECT id FROM widgets WHERE agent_id = ?)',
        agentId,
      );
      this.sql.exec('DELETE FROM widgets WHERE agent_id = ?', agentId);
      this.sql.exec('DELETE FROM idempotency WHERE agent_id = ?', agentId);
      this.sql.exec('DELETE FROM agents WHERE id = ?', agentId);
      this.tomb('agent', agentId, now);
      this.tomb('agent', `token:${a.token_hash}`, now);
      this.log('agent-deleted', agentId, now);
    });
    return true;
  }

  private deleteDashboard(now: number) {
    this.ctx.storage.transactionSync(() => {
      this.setMeta('deleted', '1');
      this.sql.exec('DELETE FROM enroll_code_macs');
      this.sql.exec('DELETE FROM events');
      this.sql.exec('DELETE FROM widgets');
      this.sql.exec('DELETE FROM idempotency');
      this.tomb('dashboard', '', now);
      this.log('dashboard-deleted', null, now);
    });
  }

  /** Drop history older than RETENTION_DAYS, old audit entries, expired codes and idempotency records. */
  prune(now: number) {
    const cutoff = now - C.RETENTION_DAYS * C.DAY_MS;
    this.sql.exec('DELETE FROM events WHERE at < ?', cutoff);
    this.sql.exec('DELETE FROM audit WHERE at < ?', cutoff);
    this.sql.exec(
      'DELETE FROM enroll_code_macs WHERE used_at IS NOT NULL OR expires_at <= ?',
      now,
    );
    this.sql.exec('DELETE FROM idempotency WHERE expires_at <= ?', now);
  }

  async alarm(): Promise<void> {
    const now = Date.now();
    this.prune(now);
    if (this.meta('owner_hash') !== null)
      await this.ctx.storage.setAlarm(now + C.PRUNE_EVERY_MS);
  }

  /** The read-only view (the Rust `Dash::view` shape). Holds no token, hash or code. */
  view(now: number) {
    this.prune(now);
    const agents = this.agents()
      .filter((a) => a.status !== 'rejected')
      .map((a) => {
        const widgets = this.sql
          .exec<WidgetRow>(
            'SELECT * FROM widgets WHERE agent_id = ? ORDER BY key',
            a.id,
          )
          .toArray()
          .map((w) => {
            const ev = this.sql
              .exec<{ at: number; data_json: string }>(
                'SELECT at, data_json FROM events WHERE widget_id = ? ORDER BY id DESC LIMIT 50',
                w.id,
              )
              .toArray()
              .reverse()
              .map((e) => ({ at: e.at, data: JSON.parse(e.data_json) }));
            return {
              key: w.key,
              type: w.type,
              title: w.title,
              state: w.state_json === null ? null : JSON.parse(w.state_json),
              events: ev,
              updatedAt: w.updated_at,
            };
          });
        return {
          id: a.id,
          name: a.name,
          description: a.description,
          fingerprint: a.fingerprint,
          status: a.status,
          heartbeat: a.heartbeat,
          lastSeen: a.last_seen,
          stale:
            a.status === 'approved' && now - a.last_seen > C.STALE_AFTER_MS,
          flagged: a.flagged === 1,
          createdAt: a.created_at,
          widgets,
        };
      });
    const audit = this.sql
      .exec<{ event: string; agent_id: string | null; at: number }>(
        'SELECT event, agent_id, at FROM audit ORDER BY id DESC LIMIT 50',
      )
      .toArray()
      .map((x) => ({ event: x.event, agentId: x.agent_id, at: x.at }));
    return {
      origin: C.DASH_ORIGIN,
      readmeVersion: C.README_VERSION,
      deleted: this.deleted,
      retentionDays: C.RETENTION_DAYS,
      agents,
      audit,
    };
  }

  /** The owner's live socket. The Worker already checked the owner key and passes its HMAC. */
  async fetch(req: Request): Promise<Response> {
    const hash = req.headers.get('x-dash-owner-hash') ?? '';
    if (!this.ownerOk(hash))
      return new Response('owner key invalid', { status: 401 });
    if (req.headers.get('upgrade')?.toLowerCase() !== 'websocket')
      return new Response('expected a websocket', { status: 426 });
    const { 0: client, 1: server } = new WebSocketPair();
    this.ctx.acceptWebSocket(server, ['owner']);
    server.send(
      JSON.stringify({ type: 'hello', readme_version: C.README_VERSION }),
    );
    return new Response(null, { status: 101, webSocket: client });
  }

  /** Read-only: anything the owner sends (but the auto-answered "ping") is ignored. */
  async webSocketMessage(): Promise<void> {}
  async webSocketClose(ws: WebSocket, code: number): Promise<void> {
    try {
      ws.close(code === 1005 ? 1000 : code, 'bye');
    } catch {
      /* already closed */
    }
  }

  /** Test seam: the stored HMACs never leave except here, for the no-raw-secret-at-rest test. */
  async dumpForTest() {
    return {
      meta: this.sql.exec('SELECT * FROM meta').toArray(),
      agents: this.sql.exec('SELECT * FROM agents').toArray(),
      codes: this.sql.exec('SELECT * FROM enroll_code_macs').toArray(),
    };
  }
}

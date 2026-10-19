/**
 * The enrollment directory: one Durable Object (`enroll`) that maps an outstanding code's HMAC to its dashboard
 * (an ENR code carries no dashboard id) and guards enrollment against guessing (Bozniak M2): per client IP 10 wrong
 * codes a minute, then 429; 50 in 24 hours, then that IP is locked out for 24 hours; and a service-wide brake.
 * Low traffic by design (codes are issued by hand), so one instance is enough. Holds only HMACs.
 */
import { DurableObject } from 'cloudflare:workers';
import * as C from './constants';
import type { Env } from './env';

export type Claim =
  | { ok: true; dashId: string }
  | { ok: false; status: 401 | 429; retryAfterMs?: number };

export class EnrollDirectory extends DurableObject<Env> {
  private sql: SqlStorage;
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    ctx.blockConcurrencyWhile(async () => {
      this.sql.exec(`
        CREATE TABLE IF NOT EXISTS codes (code_hash TEXT PRIMARY KEY, dash_id TEXT NOT NULL, expires_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS failures (ip_hash TEXT NOT NULL, at INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS failures_ip ON failures (ip_hash, at);
        CREATE TABLE IF NOT EXISTS locks (ip_hash TEXT PRIMARY KEY, until INTEGER NOT NULL);
      `);
    });
  }

  async register(
    codeHash: string,
    dashId: string,
    expiresAt: number,
  ): Promise<void> {
    this.sql.exec(
      'INSERT OR REPLACE INTO codes (code_hash, dash_id, expires_at) VALUES (?, ?, ?)',
      codeHash,
      dashId,
      expiresAt,
    );
    if ((await this.ctx.storage.getAlarm()) === null)
      await this.ctx.storage.setAlarm(Date.now() + C.PRUNE_EVERY_MS);
  }

  /** Is this IP allowed to try a code right now? */
  private gate(ipHash: string, now: number): Claim | null {
    const lock = this.sql
      .exec<{ until: number }>(
        'SELECT until FROM locks WHERE ip_hash = ?',
        ipHash,
      )
      .toArray()[0];
    if (lock && lock.until > now)
      return { ok: false, status: 429, retryAfterMs: lock.until - now };
    const win = now - 60_000;
    const mine = this.sql
      .exec<{ n: number; oldest: number | null }>(
        'SELECT COUNT(*) AS n, MIN(at) AS oldest FROM failures WHERE ip_hash = ? AND at > ?',
        ipHash,
        win,
      )
      .one();
    if (mine.n >= C.ENROLL_IP_FAILS_PER_MIN)
      return {
        ok: false,
        status: 429,
        retryAfterMs: Math.max(1000, (mine.oldest ?? now) + 60_000 - now),
      };
    const all = this.sql
      .exec<{ n: number }>(
        'SELECT COUNT(*) AS n FROM failures WHERE at > ?',
        win,
      )
      .one();
    if (all.n >= C.ENROLL_GLOBAL_FAILS_PER_MIN)
      return { ok: false, status: 429, retryAfterMs: 60_000 };
    return null;
  }

  /** Record a wrong code for this IP (also called when the dashboard itself refused a code it had rotated). */
  async fail(ipHash: string, now = Date.now()): Promise<void> {
    this.sql.exec(
      'INSERT INTO failures (ip_hash, at) VALUES (?, ?)',
      ipHash,
      now,
    );
    const day = this.sql
      .exec<{ n: number }>(
        'SELECT COUNT(*) AS n FROM failures WHERE ip_hash = ? AND at > ?',
        ipHash,
        now - C.DAY_MS,
      )
      .one().n;
    if (day >= C.ENROLL_IP_FAILS_BEFORE_LOCK)
      this.sql.exec(
        'INSERT OR REPLACE INTO locks (ip_hash, until) VALUES (?, ?)',
        ipHash,
        now + C.ENROLL_IP_LOCK_MS,
      );
    if ((await this.ctx.storage.getAlarm()) === null)
      await this.ctx.storage.setAlarm(now + C.PRUNE_EVERY_MS);
  }

  /** Take a code: single use across the service (the row is gone after this), then the dashboard checks it again. */
  async claim(
    ipHash: string,
    codeHash: string,
    now = Date.now(),
  ): Promise<Claim> {
    const gated = this.gate(ipHash, now);
    if (gated) return gated;
    const row = this.sql
      .exec<{ dash_id: string }>(
        'SELECT dash_id FROM codes WHERE code_hash = ? AND expires_at > ?',
        codeHash,
        now,
      )
      .toArray()[0];
    if (!row) {
      await this.fail(ipHash, now);
      return { ok: false, status: 401 };
    }
    this.sql.exec('DELETE FROM codes WHERE code_hash = ?', codeHash);
    return { ok: true, dashId: row.dash_id };
  }

  async alarm(): Promise<void> {
    const now = Date.now();
    this.sql.exec('DELETE FROM codes WHERE expires_at <= ?', now);
    this.sql.exec('DELETE FROM failures WHERE at <= ?', now - C.DAY_MS);
    this.sql.exec('DELETE FROM locks WHERE until <= ?', now);
    const left = this.sql
      .exec<{ n: number }>(
        'SELECT (SELECT COUNT(*) FROM codes) + (SELECT COUNT(*) FROM failures) + (SELECT COUNT(*) FROM locks) AS n',
      )
      .one().n;
    if (left > 0) await this.ctx.storage.setAlarm(now + C.PRUNE_EVERY_MS);
  }
}

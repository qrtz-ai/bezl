/**
 * The enrollment directory: one Durable Object (`enroll`) that maps an outstanding code's HMAC to its dashboard
 * (an ENR code carries no dashboard id) and guards enrollment against guessing (Bozniak M2): per client IP 10 wrong
 * codes a minute, then 429; 50 in 24 hours, then that source is locked out for 24 hours; and a service-wide brake.
 * A source is an IPv4 /32 or an IPv6 /64, with a looser count on the IPv6 /48 (src/ip.ts), so rotating addresses
 * inside one allocation gets around neither the limit nor the lockout. Because every source is capped before its
 * wrong codes are counted, one /48 adds at most ENROLL_V6_48_FAILS_PER_MIN a minute to the brake and one IPv4
 * address ENROLL_IP_FAILS_PER_MIN: tripping the brake needs many independent allocations, not one attacker.
 * Also holds the one-time setup flag (Bozniak M1). Low traffic by design (codes are issued by hand), so one
 * instance is enough. Holds only HMACs.
 *
 * QRT-284: a code is `<id>.<secret>`. Rows are found by id and hold HMAC(DASH_ENROLL_KEY, id.secret), compared in
 * constant time. Each wrong secret on an id is a strike; after ENROLL_CODE_STRIKES the row stays (until it expires)
 * but can never be claimed. Unknown id, wrong secret and dead code all take the same path: one row lookup, one
 * constant-time compare (against a dummy MAC when there is no row), one counted failure, the same 401.
 */
import { DurableObject } from 'cloudflare:workers';
import * as C from './constants';
import { ctEq } from './crypto';
import type { Env } from './env';

/** HMACs of a client's limiter buckets (see src/ip.ts): `wide` is the IPv6 /48, null for IPv4. */
export type SourceHash = { narrow: string; wide: string | null };

export type Claim =
  | { ok: true; dashId: string }
  | { ok: false; status: 401 | 429; retryAfterMs?: number };

/** Compared against when no row matches the id: 64 hex characters no HMAC output equals in practice. */
const DUMMY_MAC = '0'.repeat(64);

export class EnrollDirectory extends DurableObject<Env> {
  private sql: SqlStorage;
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    ctx.blockConcurrencyWhile(async () => {
      this.sql.exec(`
        DROP TABLE IF EXISTS codes;
        CREATE TABLE IF NOT EXISTS enroll_codes (code_id TEXT PRIMARY KEY, secret_mac TEXT NOT NULL,
          dash_id TEXT NOT NULL, expires_at INTEGER NOT NULL, strikes INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS failures (narrow TEXT NOT NULL, wide TEXT, at INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS failures_narrow ON failures (narrow, at);
        CREATE INDEX IF NOT EXISTS failures_wide ON failures (wide, at);
        CREATE INDEX IF NOT EXISTS failures_at ON failures (at);
        CREATE TABLE IF NOT EXISTS locks (bucket TEXT PRIMARY KEY, until INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
      `);
    });
  }

  async register(
    codeId: string,
    secretMac: string,
    dashId: string,
    expiresAt: number,
  ): Promise<void> {
    this.sql.exec(
      'INSERT OR REPLACE INTO enroll_codes (code_id, secret_mac, dash_id, expires_at, strikes) VALUES (?, ?, ?, ?, 0)',
      codeId,
      secretMac,
      dashId,
      expiresAt,
    );
    if ((await this.ctx.storage.getAlarm()) === null)
      await this.ctx.storage.setAlarm(Date.now() + C.PRUNE_EVERY_MS);
  }

  /**
   * Bozniak M1: the setup token creates one dashboard, ever. Atomic (a DO runs one call at a time and this has no
   * await): the first caller sets setup_used and gets true, everyone after gets false.
   */
  async useSetup(now = Date.now()): Promise<boolean> {
    const r = this.sql.exec(
      "INSERT OR IGNORE INTO meta (k, v) VALUES ('setup_used', ?)",
      String(now),
    );
    return r.rowsWritten > 0;
  }

  private lockedFor(bucket: string, now: number): number {
    const lock = this.sql
      .exec<{ until: number }>(
        'SELECT until FROM locks WHERE bucket = ?',
        bucket,
      )
      .toArray()[0];
    return lock && lock.until > now ? lock.until - now : 0;
  }

  private recent(
    col: 'narrow' | 'wide',
    bucket: string,
    since: number,
  ): { n: number; oldest: number | null } {
    return this.sql
      .exec<{ n: number; oldest: number | null }>(
        `SELECT COUNT(*) AS n, MIN(at) AS oldest FROM failures WHERE ${col} = ? AND at > ?`,
        bucket,
        since,
      )
      .one();
  }

  /** Is this source allowed to try a code right now? */
  private gate(src: SourceHash, now: number): Claim | null {
    for (const b of [src.narrow, src.wide]) {
      const left = b === null ? 0 : this.lockedFor(b, now);
      if (left > 0) return { ok: false, status: 429, retryAfterMs: left };
    }
    const win = now - 60_000;
    const limits: ['narrow' | 'wide', string | null, number][] = [
      ['narrow', src.narrow, C.ENROLL_IP_FAILS_PER_MIN],
      ['wide', src.wide, C.ENROLL_V6_48_FAILS_PER_MIN],
    ];
    for (const [col, b, max] of limits) {
      if (b === null) continue;
      const r = this.recent(col, b, win);
      if (r.n >= max)
        return {
          ok: false,
          status: 429,
          retryAfterMs: Math.max(1000, (r.oldest ?? now) + 60_000 - now),
        };
    }
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

  /** Record a wrong code for this source (also called when the dashboard itself refused a code it had rotated). */
  async fail(src: SourceHash, now = Date.now()): Promise<void> {
    this.sql.exec(
      'INSERT INTO failures (narrow, wide, at) VALUES (?, ?, ?)',
      src.narrow,
      src.wide,
      now,
    );
    const lock = (bucket: string) =>
      this.sql.exec(
        'INSERT OR REPLACE INTO locks (bucket, until) VALUES (?, ?)',
        bucket,
        now + C.ENROLL_IP_LOCK_MS,
      );
    if (
      this.recent('narrow', src.narrow, now - C.DAY_MS).n >=
      C.ENROLL_IP_FAILS_BEFORE_LOCK
    )
      lock(src.narrow);
    if (
      src.wide !== null &&
      this.recent('wide', src.wide, now - C.DAY_MS).n >=
        C.ENROLL_V6_48_FAILS_BEFORE_LOCK
    )
      lock(src.wide);
    if ((await this.ctx.storage.getAlarm()) === null)
      await this.ctx.storage.setAlarm(now + C.PRUNE_EVERY_MS);
  }

  /**
   * Take a code: single use across the service (the row is gone after this), then the dashboard checks it again.
   * A locked or rate-limited source gets 429 before any row is looked at.
   */
  async claim(
    src: SourceHash,
    codeId: string,
    secretMac: string,
    now = Date.now(),
  ): Promise<Claim> {
    const gated = this.gate(src, now);
    if (gated) return gated;
    const row = this.sql
      .exec<{ secret_mac: string; dash_id: string; strikes: number }>(
        'SELECT secret_mac, dash_id, strikes FROM enroll_codes WHERE code_id = ? AND expires_at > ?',
        codeId,
        now,
      )
      .toArray()[0];
    // Always one compare, so an unknown id costs the same as a wrong secret.
    const match = ctEq(row?.secret_mac ?? DUMMY_MAC, secretMac);
    const live = row !== undefined && row.strikes < C.ENROLL_CODE_STRIKES;
    if (!match || !live) {
      if (live && !match)
        this.sql.exec(
          'UPDATE enroll_codes SET strikes = strikes + 1 WHERE code_id = ?',
          codeId,
        );
      await this.fail(src, now);
      return { ok: false, status: 401 };
    }
    this.sql.exec('DELETE FROM enroll_codes WHERE code_id = ?', codeId);
    return { ok: true, dashId: row.dash_id };
  }

  async alarm(): Promise<void> {
    const now = Date.now();
    this.sql.exec('DELETE FROM enroll_codes WHERE expires_at <= ?', now);
    this.sql.exec('DELETE FROM failures WHERE at <= ?', now - C.DAY_MS);
    this.sql.exec('DELETE FROM locks WHERE until <= ?', now);
    const left = this.sql
      .exec<{ n: number }>(
        'SELECT (SELECT COUNT(*) FROM enroll_codes) + (SELECT COUNT(*) FROM failures) + (SELECT COUNT(*) FROM locks) AS n',
      )
      .one().n;
    if (left > 0) await this.ctx.storage.setAlarm(now + C.PRUNE_EVERY_MS);
  }
}

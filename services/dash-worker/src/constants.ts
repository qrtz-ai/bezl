/**
 * The limits of Agent Dash (ADR 0228, spec docs/dash/agent-readme.md v2.1). Twins of crates/qrtz-dash/src/lib.rs;
 * test/rust-twins.test.ts reads that file and fails if any number here drifts.
 */
export const DASH_ORIGIN = 'https://dash.qrtz.ai';
export const README_VERSION = '2.1';
export const RETENTION_DAYS = 30;
export const BATCH_MAX_OPS = 20;
export const ENROLL_TTL_MS = 15 * 60 * 1000;
export const STALE_AFTER_MS = 10 * 60 * 1000;
export const MAX_BODY_BYTES = 8 * 1024;
export const MAX_BATCH_BODY_BYTES = 64 * 1024;
export const WRITES_PER_MIN_AGENT = 60;
export const WRITES_PER_MIN_DASHBOARD = 300;
export const MAX_AGENTS = 50;
export const MAX_WIDGETS_PER_AGENT = 20;
export const LOG_RING = 500;
export const CHART_RING = 1000;
export const IDEMPOTENCY_TTL_MS = 24 * 60 * 60 * 1000;
export const DAY_MS = 24 * 60 * 60 * 1000;
/** Bozniak M2: wrong enrollment codes, dashboard-wide per minute, and in total before a forced rotation. */
export const ENROLL_FAILS_PER_MIN = 10;
export const ENROLL_FAILS_BEFORE_ROTATION = 50;
/** Server only: the same limits per client IP (a wrong code can't be pinned on a dashboard), lockout length. */
export const ENROLL_IP_FAILS_PER_MIN = 10;
export const ENROLL_IP_FAILS_BEFORE_LOCK = 50;
export const ENROLL_IP_LOCK_MS = DAY_MS;
/** Server only (Bozniak M2): the looser count on an IPv6 /48 (the per-IP limits above apply to its /64s). */
export const ENROLL_V6_48_FAILS_PER_MIN = 40;
export const ENROLL_V6_48_FAILS_BEFORE_LOCK = 200;
/** Server only: all wrong codes across the service per minute before enrollment pauses for everyone. */
export const ENROLL_GLOBAL_FAILS_PER_MIN = 600;
/** Server only: live-update pushes per owner socket, at most one per this many ms. */
export const LIVE_THROTTLE_MS = 250;
/** Server only: the retention alarm's period. */
export const PRUNE_EVERY_MS = 60 * 60 * 1000;
/** Server only (QRT-307): the stored size of one widget state or event (JSON, after scrubbing). */
export const MAX_CONTENT_BYTES = 8 * 1024;

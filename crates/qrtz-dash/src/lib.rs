//! Agent Dash (ADR 0228, spec docs/dash/agent-readme.md v2.1): a read-only dashboard that the user's agents write to.
//!
//! This crate is the whole server logic with no transport: [`Dash::handle`] takes an agent's HTTP request (method,
//! path, headers, body bytes) and returns the status and JSON body the spec names; the user's side (issue a code,
//! approve, reject, delete, the read-only view) are plain methods the app's `dash:*` IPC channels call. Time is
//! always passed in (`now_ms`) so every rule is testable. Nothing here listens on a socket.
mod blocks;
mod crypto;
mod store;

pub use blocks::{is_https_url, is_punycode_host, validate_append, validate_put, BlockError, BlockType};
pub use crypto::{hmac_sha256, Pepper};
pub use store::{Dash, Request, Response, Scope};

/// The one origin agents talk to (placeholder until the server ships; no DNS or deploy yet).
pub const DASH_ORIGIN: &str = "https://dash.qrtz.ai";
/// `readme_version` in every response.
pub const README_VERSION: &str = "2.1";
/// History older than this is dropped (per dashboard default).
pub const RETENTION_DAYS: u64 = 30;
/// Most ops in one `POST /v1/batch`.
pub const BATCH_MAX_OPS: usize = 20;
/// Enrollment codes expire this long after issue.
pub const ENROLL_TTL_MS: u64 = 15 * 60 * 1000;
/// No write or heartbeat for this long: the agent's widgets show stale.
pub const STALE_AFTER_MS: u64 = 10 * 60 * 1000;
pub const MAX_BODY_BYTES: usize = 8 * 1024;
pub const MAX_BATCH_BODY_BYTES: usize = 64 * 1024;
pub const WRITES_PER_MIN_AGENT: usize = 60;
pub const WRITES_PER_MIN_DASHBOARD: usize = 300;
/// Wrong enrollment codes per minute, dashboard-wide, before 429.
pub const ENROLL_FAILS_PER_MIN: usize = 10;
/// Wrong codes since the last rotation before every outstanding code is revoked (forced rotation).
pub const ENROLL_FAILS_BEFORE_ROTATION: usize = 50;
pub const MAX_AGENTS: usize = 50;
pub const MAX_WIDGETS_PER_AGENT: usize = 20;
pub const LOG_RING: usize = 500;
pub const CHART_RING: usize = 1000;
pub const IDEMPOTENCY_TTL_MS: u64 = 24 * 60 * 60 * 1000;
pub const DAY_MS: u64 = 24 * 60 * 60 * 1000;

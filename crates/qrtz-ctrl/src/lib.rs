//! qrtz ctrl core (slice 1, ADR 0100: ctrl never takes over the UI).
//!
//! ctrl runs typed tasks in the background while the user keeps working. This crate holds the parts that are pure
//! and testable on Linux:
//!
//! - [`ladder`]: every action arrives with the routes that could perform it. The ladder picks the lowest rung that
//!   can run without touching the user's screen, mouse or keyboard (files → workspace → app scripting → background
//!   AX → per-process key events). Anything that needs focus, the cursor or activation is refused, never attempted.
//! - [`guard`]: snapshots of what the user owns (frontmost app, cursor, focused element, key window) taken before and
//!   after every action, and the comparison that turns a difference into a violation.
//! - [`log`]: the append-only, hash-chained event log; state is a pure fold over it (replay, scrub, fork).
//! - [`runner`]: the effect that ties them together: decide, snapshot, execute, snapshot, record.
//! - [`fs_exec`]: the first executor, plain file moves (downloads cleanup needs no UI at all).
//! - [`task`], [`store`], [`service`] (slice 2, ADR 0103): typed tasks with a schedule, persisted as append-only
//!   hash-chained files in the app data dir, and the `ctrl:*` channels the shell's ctrl view calls.
//! - [`schedule`] (slice 3, ADR 0105): next-due math against the local zone (DST gap/overlap rules), used by the
//!   in-process scheduler ticker on [`service::CtrlService`].
//!
//! The macOS side (real probes, AX, `CGEventPostToPid`, `NSWorkspace`) is `native/qrtz-helper/Sources/QrtzCtrl`.
//! Both sides test their policy tables and guard semantics against the same fixtures in
//! `native/qrtz-helper/Fixtures/ctrl/`.
pub mod fs_exec;
pub mod guard;
pub mod ladder;
pub mod log;
pub mod policy;
pub mod runner;
pub mod schedule;
pub mod service;
pub mod store;
pub mod task;

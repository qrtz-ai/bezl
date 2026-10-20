//! qrtz ctrl core: **ctrl do** (ADR 0214, which amends ADR 0100 and supersedes the per-chore ctrl ADRs).
//!
//! Jared types one command ("open youtube", "create a new note titled groceries") and ctrl does it in the
//! foreground, the way a person would, through a chain of small decisions over the macOS accessibility tree
//! (`docs/ctrl/decision-layer.md`, Jared's Jev-style design):
//!
//! choose the app → choose a move (a menu-bar shortcut, the built-in shortcut table, or a branch of the window's
//! tree) → descend → choose an element action → choose the text → act → verify against the tree diff, and loop
//! within a step budget. Every question has at most 20 options including "none of these"; a calibrated probability
//! acts at p ≥ 0.8, tries with backtracking at p ≥ 0.5, and otherwise asks Jared to pick from the top three.
//!
//! - [`ax`]: the tree as the helper sends it (wire) and as the log holds it (hashed tokens only), pruning, grouping
//!   to ≤ 20 options, the tree hash and diff. Secure, password and payment fields never enter it.
//! - [`chain`]: the option sources of every node and the ranks that prune them to ≤ 20 (never a decision: QRT-293
//!   removed the rules scorer; the downloaded model decides), a pure function of the goal and the state.
//! - [`decide`]: scoring, calibration, act / tentative / ask, backtracking, replay check.
//! - [`act`]: the fixed action set and its policy (no free-form AppleScript, no coordinate clicks).
//! - [`gate`]: the host-owned confirm rules table; approvals bind to (element handle, action, tree hash).
//! - [`guard`]: the stop key (Esc) and "user input wins".
//! - [`log`], [`store`]: the append-only hash-chained run log (a pure fold) and the `ctrl:do` store.
//! - [`runner`], [`driver`]: one guarded action, and the loop that runs the chain through the helper.
//! - [`helper`], [`laya`]: the `ctrl.*` helper protocol (`ctrl.tree`, `ctrl.menu.shortcuts`, `ctrl.ax.act`,
//!   `ctrl.observe`, plus `ctrl.catalog` and Laya) and the Laya scorer.
//! - [`service`]: the `ctrl:*` channels the shell's ctrl view calls.
//!
//! The macOS side (the tree reader, the actor and the stop-key watcher) is `native/qrtz-helper/Sources/QrtzCtrl`.
#![forbid(unsafe_code)]
pub mod act;
pub mod ax;
pub mod chain;
pub mod decide;
pub mod driver;
pub mod gate;
pub mod guard;
pub mod helper;
pub mod laya;
pub mod log;
pub mod runner;
#[rustfmt::skip]
pub mod secure_words;
pub mod service;
pub mod store;

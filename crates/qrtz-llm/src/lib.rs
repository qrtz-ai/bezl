//! qrtz meeting summaries: the on-device extractive fallback and the summary [`StepExecutor`] (ADR 0042).
//! See `docs/rust-core-api.md` §qrtz-llm.
//!
//! [`StepExecutor`]: qrtz_lifecycle::runtime::StepExecutor
pub mod extractive;
pub mod js;
pub mod steps;
pub mod templates;

pub use extractive::{extractive_summary, split_sentences};
pub use steps::LlmSteps;
pub use templates::{template_by_id, SummaryTemplate, SUMMARY_TEMPLATES};

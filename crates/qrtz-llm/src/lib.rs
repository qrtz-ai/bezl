//! qrtz meeting summaries, on-device only: the summary prompt and reply parser, the extractive summary and
//! [`load_sync`] (what a summary is built from). The summary step is qrtz-app's `SummarySteps` (ADR 0075, Apple
//! Intelligence through the helper). There is no cloud client and no network (ADR 0076). The chat answer
//! (ADR 0059) was removed with the chat (ADR 0073). See `docs/rust-core-api.md` §qrtz-llm.
pub mod extractive;
pub mod js;
pub mod prompt;
pub mod steps;
pub mod templates;

pub use extractive::{extractive_summary, split_sentences};
pub use prompt::{build_summary_prompt, parse_summary_reply, SummaryOptions};
pub use steps::{load_sync, Loaded};
pub use templates::{template_by_id, SummaryTemplate, SUMMARY_TEMPLATES};

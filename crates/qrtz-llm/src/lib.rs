//! qrtz meeting summaries: the OpenRouter client, prompts, the on-device extractive fallback and the summary
//! [`StepExecutor`] (ADR 0042). The chat answer (ADR 0059) was removed with the chat (ADR 0073).
//! See `docs/rust-core-api.md` §qrtz-llm.
//!
//! [`StepExecutor`]: qrtz_lifecycle::runtime::StepExecutor
pub mod extractive;
pub mod js;
pub mod prompt;
pub mod provider;
pub mod secrets;
pub mod steps;
pub mod templates;

pub use extractive::{extractive_summary, split_sentences};
pub use prompt::{build_summary_prompt, parse_summary_reply, SummaryOptions};
pub use provider::{
    create_provider, http_client, needs_credits, LlmProvider, ProviderId, OPENROUTER_DEFAULT_MODEL,
};
pub use secrets::{EnvOverride, Secret, SecretSource};
pub use steps::{LlmConfig, LlmSteps, SUMMARY_DEADLINE_MS};
pub use templates::{template_by_id, SummaryTemplate, SUMMARY_TEMPLATES};

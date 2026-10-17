//! qrtz meeting summaries and grounded chat: the OpenRouter client, prompts, the on-device extractive
//! fallback, the summary [`StepExecutor`], and chat answers (ADR 0042, ADR 0059).
//! See `docs/rust-core-api.md` §qrtz-llm.
//!
//! [`StepExecutor`]: qrtz_lifecycle::runtime::StepExecutor
pub mod chat;
pub mod extractive;
pub mod js;
pub mod prompt;
pub mod provider;
pub mod secrets;
pub mod steps;
pub mod templates;

pub use chat::{
    answer_question, build_chat_prompt, extractive_answer, quote_answer, ChatAnswer, ChatSource, ChatTurn,
    ChatTurnRole, CHAT_DEADLINE_MS, CHAT_EXTRACTIVE_LINES, CHAT_HISTORY_TURNS, CHAT_PROMPT_SOURCES,
};
pub use extractive::{extractive_summary, split_sentences};
pub use prompt::{build_summary_prompt, parse_summary_reply, SummaryOptions};
pub use provider::{
    create_provider, http_client, needs_credits, LlmProvider, ProviderId, OPENROUTER_DEFAULT_MODEL,
};
pub use secrets::{EnvOverride, Secret, SecretSource};
pub use steps::{LlmConfig, LlmSteps, SUMMARY_DEADLINE_MS};
pub use templates::{template_by_id, SummaryTemplate, SUMMARY_TEMPLATES};

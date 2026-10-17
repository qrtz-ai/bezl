//! note:chat (N89, ADR 0059): `qrtz_search::retrieve` (hybrid order when the model is ready, else
//! `rank_sources(question, candidates, 24)`), then `LlmSteps::answer_chat` with the same provider config, keys and
//! `qrtz_llm::http_client()` as the summary. No provider, an error or a timeout gives the extractive answer with
//! `llmError`.
use crate::ports::ChatPort;
use async_trait::async_trait;
use qrtz_ipc::{AppError, ErrorCode};
use qrtz_llm::chat::{ChatTurn, CHAT_PROMPT_SOURCES};
use qrtz_model::LifecycleError;
use qrtz_storage::StorageHandle;
use serde_json::Value;
use std::sync::Arc;

pub struct NoteChat {
    pub db: StorageHandle,
    pub embedder: Arc<qrtz_search::LazyEmbedder>,
    pub llm: Arc<qrtz_llm::LlmSteps<StorageHandle>>,
}

fn app_error(e: LifecycleError) -> AppError {
    let code = serde_json::from_value::<ErrorCode>(Value::String(e.code)).unwrap_or(ErrorCode::Internal);
    AppError::new(code, e.message).retryable(e.retryable)
}

#[async_trait]
impl ChatPort for NoteChat {
    async fn chat(&self, p: Value) -> Result<Value, AppError> {
        // The router validated the request against the contract (question 1..2000, history <= 20).
        let question = p["question"].as_str().unwrap_or_default().to_owned();
        let meeting_id = p["meetingId"].as_str().map(str::to_owned);
        let history: Vec<ChatTurn> = match p.get("history") {
            Some(h) if !h.is_null() => serde_json::from_value(h.clone())
                .map_err(|e| AppError::new(ErrorCode::Validation, format!("note:chat history: {e}")))?,
            _ => vec![],
        };
        let r = qrtz_search::retrieve(&self.db, &self.embedder, &question, meeting_id)
            .await
            .map_err(app_error)?;
        let sources = r
            .ranked
            .unwrap_or_else(|| qrtz_search::rank_sources(&question, &r.candidates, CHAT_PROMPT_SOURCES));
        let answer = self.llm.answer_chat(&question, &sources, &history).await;
        serde_json::to_value(answer).map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))
    }
}

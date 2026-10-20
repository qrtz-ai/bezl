use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Mirrors `ErrorCode` in packages/core/src/errors.ts (checked against contracts/ipc-contract.json in tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    #[serde(rename = "E_VALIDATION")]
    Validation,
    #[serde(rename = "E_UNKNOWN_CHANNEL")]
    UnknownChannel,
    #[serde(rename = "E_VERSION")]
    Version,
    #[serde(rename = "E_FORBIDDEN")]
    Forbidden,
    #[serde(rename = "E_NOT_FOUND")]
    NotFound,
    #[serde(rename = "E_TIMEOUT")]
    Timeout,
    #[serde(rename = "E_UNAVAILABLE")]
    Unavailable,
    #[serde(rename = "E_PROVIDER")]
    Provider,
    #[serde(rename = "E_INTERNAL")]
    Internal,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 9] = [
        ErrorCode::Validation,
        ErrorCode::UnknownChannel,
        ErrorCode::Version,
        ErrorCode::Forbidden,
        ErrorCode::NotFound,
        ErrorCode::Timeout,
        ErrorCode::Unavailable,
        ErrorCode::Provider,
        ErrorCode::Internal,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Validation => "E_VALIDATION",
            ErrorCode::UnknownChannel => "E_UNKNOWN_CHANNEL",
            ErrorCode::Version => "E_VERSION",
            ErrorCode::Forbidden => "E_FORBIDDEN",
            ErrorCode::NotFound => "E_NOT_FOUND",
            ErrorCode::Timeout => "E_TIMEOUT",
            ErrorCode::Unavailable => "E_UNAVAILABLE",
            ErrorCode::Provider => "E_PROVIDER",
            ErrorCode::Internal => "E_INTERNAL",
        }
    }
}

/// The only error that crosses the IPC boundary. Serializes exactly like `AppError.toJSON()`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{}: {}", code.as_str(), message)]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: Option<bool>,
    pub details: Option<Value>,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: None,
            details: None,
        }
    }
    pub fn retryable(mut self, r: bool) -> Self {
        self.retryable = Some(r);
        self
    }
    pub fn details(mut self, d: Value) -> Self {
        self.details = Some(d);
        self
    }
    /// Same default as TS: timeouts and unavailability are retryable.
    pub fn is_retryable(&self) -> bool {
        self.retryable
            .unwrap_or(matches!(self.code, ErrorCode::Timeout | ErrorCode::Unavailable))
    }
    pub fn to_json(&self) -> Value {
        let mut v = serde_json::json!({
            "name": "AppError",
            "code": self.code.as_str(),
            "message": self.message,
            "retryable": self.is_retryable(),
        });
        if let Some(d) = &self.details {
            v["details"] = d.clone();
        }
        v
    }
}

//! Transport-agnostic, fully validated IPC dispatcher. A line-for-line port of
//! packages/core/src/ipc/router.ts: envelope → channel → version → sub app → capability →
//! request schema → handler → response schema. Never panics, never returns Err: failures are
//! `{ok:false, id, error}` results exactly like the TS router.
use crate::contract::{contract, issue_paths, issues, Contract, RequestSpec};
use crate::error::{AppError, ErrorCode};
use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct CallContext {
    pub sub_app: String,
    pub request_id: String,
}

#[async_trait]
pub trait Handler: Send + Sync {
    /// `payload` has passed the channel's request schema (absent payloads arrive as `Value::Null`).
    async fn handle(&self, channel: &str, payload: Value, ctx: &CallContext) -> Result<Value, AppError>;
}

pub fn no_handler(channel: &str) -> AppError {
    AppError::new(ErrorCode::Unavailable, format!("No handler for \"{channel}\""))
}

pub struct Router<H: Handler> {
    contract: &'static Contract,
    handler: H,
    validate_responses: bool,
}

impl<H: Handler> Router<H> {
    pub fn new(handler: H) -> Self {
        Self {
            contract: contract(),
            handler,
            validate_responses: true,
        }
    }
    pub fn validate_responses(mut self, on: bool) -> Self {
        self.validate_responses = on;
        self
    }
    pub fn handler(&self) -> &H {
        &self.handler
    }

    pub async fn dispatch(&self, raw: Value) -> Value {
        let raw_id = raw
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        match self.try_dispatch(raw).await {
            Ok((id, data)) => json!({ "ok": true, "id": id, "data": data }),
            Err(e) => {
                if e.code == ErrorCode::Internal {
                    tracing::error!(id = %raw_id, code = e.code.as_str(), err = %e.message, "ipc call failed");
                } else {
                    tracing::warn!(id = %raw_id, code = e.code.as_str(), err = %e.message, "ipc call failed");
                }
                json!({ "ok": false, "id": raw_id, "error": e.to_json() })
            }
        }
    }

    async fn try_dispatch(&self, mut raw: Value) -> Result<(String, Value), AppError> {
        let c = self.contract;
        // `payload: undefined` is dropped by JSON serialization; zod treats absent and undefined alike.
        if let Some(obj) = raw.as_object_mut() {
            obj.entry("payload").or_insert(Value::Null);
        }
        if !c.envelope.is_valid(&raw) {
            return Err(AppError::new(ErrorCode::Validation, "Malformed IPC envelope"));
        }
        let id = raw["id"].as_str().unwrap_or_default().to_owned();
        let channel = raw["channel"].as_str().unwrap_or_default();
        let version = raw["version"].as_u64().unwrap_or(0);
        let sub_app = raw["subApp"].as_str().unwrap_or_default();
        let payload = raw.get("payload").cloned().unwrap_or(Value::Null);

        let def = c.channels.get(channel).ok_or_else(|| {
            AppError::new(
                ErrorCode::UnknownChannel,
                format!("Unknown channel \"{channel}\""),
            )
        })?;
        if version != def.version {
            return Err(AppError::new(
                ErrorCode::Version,
                format!(
                    "Channel \"{channel}\" is v{}, caller sent v{version}",
                    def.version
                ),
            ));
        }
        if !c.manifests.contains_key(sub_app) {
            return Err(AppError::new(
                ErrorCode::Forbidden,
                format!("Unknown sub app \"{sub_app}\""),
            ));
        }
        if !c.has_capability(sub_app, &def.capability) {
            return Err(AppError::new(
                ErrorCode::Forbidden,
                format!("Sub app \"{sub_app}\" lacks capability \"{}\"", def.capability),
            ));
        }
        let invalid = |details: Vec<Value>| {
            AppError::new(
                ErrorCode::Validation,
                format!("Invalid request for \"{channel}\""),
            )
            .details(Value::Array(details))
        };
        match &def.request {
            RequestSpec::Void => {
                if !payload.is_null() {
                    return Err(invalid(vec![
                        json!({"path": "", "message": "expected no payload"}),
                    ]));
                }
            }
            RequestSpec::Schema { optional, validator } => {
                let absent_ok = payload.is_null() && *optional;
                if !absent_ok && !validator.is_valid(&payload) {
                    return Err(invalid(issues(validator, &payload)));
                }
            }
        }
        let ctx = CallContext {
            sub_app: sub_app.to_owned(),
            request_id: id.clone(),
        };
        let data = self.handler.handle(channel, payload, &ctx).await?;
        if self.validate_responses && !def.response.is_valid(&data) {
            // ADR 0083: the paths only; an issue's message quotes the value (a transcript, a summary).
            tracing::error!(channel, at = ?issue_paths(&def.response, &data), "invalid handler response");
            return Err(AppError::new(
                ErrorCode::Internal,
                format!("Handler for \"{channel}\" returned invalid data"),
            ));
        }
        Ok((id, data))
    }
}

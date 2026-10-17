//! LLM providers over HTTP (port of `post`, `openAiCompatible`, `anthropic`, `google`, `createProvider` and
//! `needsCredits` in packages/core/src/note/llm.ts). Failure classification matches TS (spec/vectors/llm/replies.json):
//! transport failures and failures after a 200 (a reset mid-body, HTML from a captive portal, an error object in a
//! 200, a reply that is not JSON) stay retryable (N8, b917847); HTTP 5xx/429 are retryable, other 4xx are not.
//!
//! The API key goes into one request header and nowhere else: it is never logged, and any error text that echoes it
//! is redacted ([`redact`]).
use std::sync::LazyLock;

use async_trait::async_trait;
use qrtz_model::LifecycleError;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::js::cut_utf16;
use crate::prompt::{ChatMessage, Role};
use crate::secrets::Secret;

/// Default model for every LLM use in qrtz via OpenRouter (TS `OPENROUTER_DEFAULT_MODEL`, "Haiku 5.5"). The
/// `note.llmModel` config overrides it.
pub const OPENROUTER_DEFAULT_MODEL: &str = "anthropic/claude-haiku-5.5";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Openai,
    Anthropic,
    Google,
    Openrouter,
    Ollama,
    Nvidia,
}

pub struct ProviderDefaults {
    pub base_url: &'static str,
    pub model: &'static str,
    /// Secret name in the secrets store; `None`: no key needed (local Ollama).
    pub secret: Option<&'static str>,
}

impl ProviderId {
    pub fn parse(s: &str) -> Option<ProviderId> {
        Some(match s {
            "openai" => ProviderId::Openai,
            "anthropic" => ProviderId::Anthropic,
            "google" => ProviderId::Google,
            "openrouter" => ProviderId::Openrouter,
            "ollama" => ProviderId::Ollama,
            "nvidia" => ProviderId::Nvidia,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::Openai => "openai",
            ProviderId::Anthropic => "anthropic",
            ProviderId::Google => "google",
            ProviderId::Openrouter => "openrouter",
            ProviderId::Ollama => "ollama",
            ProviderId::Nvidia => "nvidia",
        }
    }
    /// `PROVIDER_DEFAULTS`.
    pub fn defaults(self) -> ProviderDefaults {
        let (base_url, model, secret) = match self {
            ProviderId::Openai => ("https://api.openai.com/v1", "gpt-4.1-mini", Some("llm.openai")),
            ProviderId::Openrouter => (
                "https://openrouter.ai/api/v1",
                OPENROUTER_DEFAULT_MODEL,
                Some("llm.openrouter"),
            ),
            ProviderId::Nvidia => (
                "https://integrate.api.nvidia.com/v1",
                "nvidia/llama-3.3-nemotron-super-49b-v1",
                Some("asr.nvidia"),
            ),
            ProviderId::Ollama => ("http://127.0.0.1:11434/v1", "llama3.2", None),
            ProviderId::Anthropic => (
                "https://api.anthropic.com/v1",
                "claude-sonnet-4-5",
                Some("llm.anthropic"),
            ),
            ProviderId::Google => (
                "https://generativelanguage.googleapis.com/v1beta",
                "gemini-2.5-flash",
                Some("llm.google"),
            ),
        };
        ProviderDefaults {
            base_url,
            model,
            secret,
        }
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// The provider id stored on the summary (`openrouter`, ...).
    fn id(&self) -> &str;
    async fn complete(&self, messages: &[ChatMessage], json: bool) -> Result<String, LifecycleError>;
}

fn err(code: &str, message: String, retryable: bool) -> LifecycleError {
    LifecycleError {
        code: code.into(),
        message,
        retryable,
    }
}

/// Replaces every occurrence of the key in an error message (a gateway that echoes the request, say).
pub fn redact(mut e: LifecycleError, key: Option<&Secret>) -> LifecycleError {
    if let Some(k) = key.map(Secret::expose).filter(|k| k.len() >= 4) {
        if e.message.contains(k) {
            e.message = e.message.replace(k, "[redacted]");
        }
    }
    e
}

/// The error and its sources on one line (reqwest's own Display hides the cause).
struct ErrChain<'a>(&'a (dyn std::error::Error + 'static));
impl std::fmt::Display for ErrChain<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)?;
        let mut s = self.0.source();
        while let Some(e) = s {
            write!(f, ": {e}")?;
            s = e.source();
        }
        Ok(())
    }
}

/// A transport-level failure: E_TIMEOUT for a timeout, else E_UNAVAILABLE. Always retryable.
fn transport_error(e: &reqwest::Error, what: &str) -> LifecycleError {
    let msg = ErrChain(e).to_string();
    err(
        if e.is_timeout() {
            "E_TIMEOUT"
        } else {
            "E_UNAVAILABLE"
        },
        format!("{what}: {}", cut_utf16(&msg, 200)),
        true,
    )
}

/// Largest reply body read after a 200 (a summary or chat answer is a few KB; ADR 0053, N81).
pub const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;
/// Largest error body read (only its first 200 characters are kept).
pub const MAX_ERROR_BYTES: usize = 64 * 1024;

/// The HTTP client every LLM call goes through. It never follows a redirect (N81, ADR 0053): a 3xx would re-send
/// the transcript (307/308) and the `x-api-key` / `x-goog-api-key` header (reqwest strips only `authorization`
/// across hosts) to wherever the `Location` points. A provider API that moves is a configuration error, reported
/// as one.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("TLS backend available")
}

/// Reads at most `cap` bytes of the body; `Ok(None)` when it is longer (the rest is never buffered).
async fn read_capped(mut res: reqwest::Response, cap: usize) -> Result<Option<Vec<u8>>, reqwest::Error> {
    if res.content_length().is_some_and(|n| n > cap as u64) {
        return Ok(None);
    }
    let mut out = Vec::new();
    while let Some(c) = res.chunk().await? {
        if out.len() + c.len() > cap {
            return Ok(None);
        }
        out.extend_from_slice(&c);
    }
    Ok(Some(out))
}

/// `post()`: one JSON POST, classified like TS.
pub(crate) async fn post(
    client: &reqwest::Client,
    url: &str,
    headers: &[(&str, String)],
    body: String,
) -> Result<Value, LifecycleError> {
    let mut req = client.post(url).header("content-type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let res = req
        .body(body)
        .send()
        .await
        .map_err(|e| transport_error(&e.without_url(), "LLM network error"))?;
    let status = res.status().as_u16();
    if !res.status().is_success() {
        let redirect = res.status().is_redirection();
        let t = read_capped(res, MAX_ERROR_BYTES)
            .await
            .ok()
            .flatten()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let mut detail = cut_utf16(&t, 200).to_string();
        if let Ok(j) = serde_json::from_str::<Value>(&t) {
            let m = match j.get("error") {
                Some(Value::String(s)) => Some(s.as_str()),
                Some(Value::Object(o)) => o.get("message").and_then(Value::as_str),
                _ => None,
            };
            if let Some(m) = m.filter(|m| !m.is_empty()) {
                detail = cut_utf16(m, 200).to_string();
            }
        }
        if redirect {
            // Not followed, and the target is not echoed (it reaches the renderer as `llmError`).
            return Err(err(
                "E_PROVIDER",
                format!("LLM HTTP {status}: redirect not followed (check the provider's base URL)"),
                false,
            ));
        }
        let hint = match status {
            401 | 403 => " (check the API key in Settings)",
            402 => " (no OpenRouter credits: add credits at openrouter.ai/settings/credits, then retry)",
            429 => " (rate limited; try again shortly)",
            _ => "",
        };
        return Err(err(
            "E_PROVIDER",
            format!("LLM HTTP {status}: {detail}{hint}"),
            status >= 500 || status == 429,
        ));
    }
    // The body is still on the wire after a 200: a reset mid-body stays retryable (N8).
    let bytes = read_capped(res, MAX_REPLY_BYTES)
        .await
        .map_err(|e| transport_error(&e.without_url(), "LLM network error while reading the reply"))?
        .ok_or_else(|| {
            err(
                "E_PROVIDER",
                format!("LLM reply was over {} MiB", MAX_REPLY_BYTES >> 20),
                false,
            )
        })?;
    let text = String::from_utf8_lossy(&bytes);
    // `Response.json()` decodes UTF-8 and drops a leading BOM.
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    serde_json::from_str::<Value>(text).map_err(|e| {
        err(
            "E_PROVIDER",
            format!("LLM reply was not JSON: {}", cut_utf16(&e.to_string(), 200)),
            true,
        )
    })
}

/// OpenRouter (and some gateways) report upstream failures as `{"error":{...}}` inside a 200.
fn error_in_body(j: &Value) -> Option<LifecycleError> {
    let e = j.as_object()?.get("error")?;
    if e.is_null() {
        return None;
    }
    let message = match e {
        Value::String(s) => s.as_str(),
        Value::Object(o) => o
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("provider error"),
        _ => "provider error",
    };
    let code = e.as_object().and_then(|o| o.get("code")).and_then(Value::as_f64);
    Some(err(
        "E_PROVIDER",
        format!("LLM error: {}", cut_utf16(message, 200)),
        // An upstream 4xx (bad request, no credits) won't fix itself; anything else might.
        code.is_none_or(|c| c >= 500.0 || c == 429.0),
    ))
}

#[derive(Serialize)]
struct Msg<'a> {
    role: &'a Role,
    content: &'a str,
}

/// The request body, keys in the TS order (`JSON.stringify({model, messages, temperature, response_format?})`).
#[derive(Serialize)]
struct OpenAiBody<'a> {
    model: &'a str,
    messages: Vec<Msg<'a>>,
    temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}
#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

/// The exact body `openAiCompatible` posts (shared vectors pin it byte for byte).
pub fn openai_request_body(model: &str, messages: &[ChatMessage], json: bool) -> String {
    serde_json::to_string(&OpenAiBody {
        model,
        messages: messages
            .iter()
            .map(|m| Msg {
                role: &m.role,
                content: &m.content,
            })
            .collect(),
        temperature: 0.2,
        response_format: json.then_some(ResponseFormat { kind: "json_object" }),
    })
    .expect("serializable")
}

/// OpenAI Chat Completions–compatible (OpenAI, OpenRouter, NVIDIA NIM, Ollama, ...).
pub struct OpenAiCompatible {
    pub id: String,
    pub client: reqwest::Client,
    pub base_url: String,
    pub api_key: Option<Secret>,
    pub model: String,
    pub extra_headers: Vec<(&'static str, String)>,
}

#[async_trait]
impl LlmProvider for OpenAiCompatible {
    fn id(&self) -> &str {
        &self.id
    }
    async fn complete(&self, messages: &[ChatMessage], json: bool) -> Result<String, LifecycleError> {
        let mut headers = self.extra_headers.clone();
        if let Some(k) = &self.api_key {
            headers.push(("authorization", format!("Bearer {}", k.expose())));
        }
        let url = format!("{}/chat/completions", self.base_url);
        let r = post(
            &self.client,
            &url,
            &headers,
            openai_request_body(&self.model, messages, json),
        )
        .await;
        let j = r.map_err(|e| redact(e, self.api_key.as_ref()))?;
        let choices = j.get("choices").and_then(Value::as_array);
        if choices.is_none_or(|c| c.is_empty()) {
            if let Some(e) = error_in_body(&j) {
                return Err(redact(e, self.api_key.as_ref()));
            }
        }
        Ok(choices
            .and_then(|c| c.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }
}

fn system_of(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct Anthropic {
    pub client: reqwest::Client,
    pub base_url: String,
    pub api_key: Secret,
    pub model: String,
}

#[derive(Serialize)]
struct AnthropicBody<'a> {
    model: &'a str,
    max_tokens: u32,
    system: String,
    messages: Vec<Msg<'a>>,
}

#[async_trait]
impl LlmProvider for Anthropic {
    fn id(&self) -> &str {
        "anthropic"
    }
    async fn complete(&self, messages: &[ChatMessage], _json: bool) -> Result<String, LifecycleError> {
        let body = serde_json::to_string(&AnthropicBody {
            model: &self.model,
            max_tokens: 2048,
            system: system_of(messages),
            messages: messages
                .iter()
                .filter(|m| m.role != Role::System)
                .map(|m| Msg {
                    role: &m.role,
                    content: &m.content,
                })
                .collect(),
        })
        .expect("serializable");
        let headers = [
            ("x-api-key", self.api_key.expose().to_string()),
            ("anthropic-version", "2023-06-01".to_string()),
        ];
        let j = post(
            &self.client,
            &format!("{}/messages", self.base_url),
            &headers,
            body,
        )
        .await
        .map_err(|e| redact(e, Some(&self.api_key)))?;
        Ok(j.get("content")
            .and_then(Value::as_array)
            .and_then(|c| {
                c.iter()
                    .find(|x| x.get("type").and_then(Value::as_str) == Some("text"))
            })
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }
}

pub struct Google {
    pub client: reqwest::Client,
    pub base_url: String,
    pub api_key: Secret,
    pub model: String,
}

#[async_trait]
impl LlmProvider for Google {
    fn id(&self) -> &str {
        "google"
    }
    async fn complete(&self, messages: &[ChatMessage], json: bool) -> Result<String, LifecycleError> {
        let contents: Vec<Value> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| {
                serde_json::json!({
                    "role": if m.role == Role::Assistant { "model" } else { "user" },
                    "parts": [{"text": m.content}],
                })
            })
            .collect();
        let mut gen = serde_json::Map::new();
        if json {
            gen.insert("responseMimeType".into(), "application/json".into());
        }
        let body = serde_json::json!({
            "systemInstruction": {"parts": [{"text": system_of(messages)}]},
            "contents": contents,
            "generationConfig": gen,
        })
        .to_string();
        let url = format!("{}/models/{}:generateContent", self.base_url, self.model);
        let headers = [("x-goog-api-key", self.api_key.expose().to_string())];
        let j = post(&self.client, &url, &headers, body)
            .await
            .map_err(|e| redact(e, Some(&self.api_key)))?;
        Ok(j.get("candidates")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("content"))
            .and_then(|c| c.get("parts"))
            .and_then(Value::as_array)
            .map(|ps| {
                ps.iter()
                    .map(|p| p.get("text").and_then(Value::as_str).unwrap_or(""))
                    .collect::<String>()
            })
            .unwrap_or_default())
    }
}

static CREDITS_402: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"LLM HTTP 402(?-u:\b)").unwrap());
static CREDITS_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u:\b(credits?|insufficient|requires? (?:more )?payment)\b)").unwrap());

/// True when an error means "this account can't pay for that model" (not a bad key or an outage).
pub fn needs_credits(message: &str) -> bool {
    CREDITS_402.is_match(message) || CREDITS_WORDS.is_match(message)
}

/// `createProvider`: `None` when no usable provider is configured (`provider` "none" or unknown, or a provider
/// that needs a key without one). No silent model swap: a 402 surfaces as an error.
pub fn create_provider(
    client: &reqwest::Client,
    provider: &str,
    api_key: Option<Secret>,
    model: Option<&str>,
    base_url: Option<&str>,
) -> Option<Box<dyn LlmProvider>> {
    let id = ProviderId::parse(provider)?;
    let d = id.defaults();
    let model = model.filter(|m| !m.is_empty()).unwrap_or(d.model).to_string();
    let base_url = base_url
        .filter(|b| !b.is_empty())
        .unwrap_or(d.base_url)
        .to_string();
    let api_key = api_key.filter(|k| !k.expose().is_empty());
    if d.secret.is_some() && api_key.is_none() {
        return None;
    }
    let client = client.clone();
    Some(match id {
        ProviderId::Anthropic => Box::new(Anthropic {
            client,
            base_url,
            api_key: api_key?,
            model,
        }),
        ProviderId::Google => Box::new(Google {
            client,
            base_url,
            api_key: api_key?,
            model,
        }),
        _ => Box::new(OpenAiCompatible {
            id: id.as_str().into(),
            client,
            base_url,
            api_key,
            model,
            extra_headers: if id == ProviderId::Openrouter {
                vec![
                    ("HTTP-Referer", "https://qrtz.ai".into()),
                    ("X-Title", "qrtz".into()),
                ]
            } else {
                vec![]
            },
        }),
    })
}

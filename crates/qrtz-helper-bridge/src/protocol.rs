//! The helper wire protocol (newline-delimited JSON over stdio), unchanged from Electron. Source of truth is
//! packages/core/src/helper/protocol.ts, rendered to contracts/helper-protocol.json.
use jsonschema::Validator;
use serde_json::Value;
use std::sync::OnceLock;

pub const PROTOCOL_JSON: &str = include_str!("../../../contracts/helper-protocol.json");
/// Lines longer than this are dropped (createLineSplitter's maxLine).
pub const MAX_LINE: usize = 1_000_000;

pub struct Protocol {
    pub version: u64,
    pub helper_to_host_types: Vec<String>,
    pub host_to_helper_types: Vec<String>,
    helper_to_host: Validator,
    host_to_helper: Validator,
}

#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "embedded at build time; a bad file is a build defect caught by its unit tests"
)]
pub fn protocol() -> &'static Protocol {
    static P: OnceLock<Protocol> = OnceLock::new();
    P.get_or_init(|| {
        let raw: Value = serde_json::from_str(PROTOCOL_JSON).expect("helper-protocol.json");
        let types = |k: &str| -> Vec<String> {
            raw[k]
                .as_array()
                .expect(k)
                .iter()
                .map(|t| t.as_str().unwrap().to_owned())
                .collect()
        };
        Protocol {
            version: raw["protocolVersion"].as_u64().expect("protocolVersion"),
            helper_to_host_types: types("helperToHostTypes"),
            host_to_helper_types: types("hostToHelperTypes"),
            helper_to_host: jsonschema::validator_for(&raw["helperToHost"]).expect("helperToHost schema"),
            host_to_helper: jsonschema::validator_for(&raw["hostToHelper"]).expect("hostToHelper schema"),
        }
    })
}

/// Parse one helper stdout line (port of parseHelperLine). Never panics.
pub fn parse_helper_line(line: &str) -> Result<Value, String> {
    let t = line.trim();
    if t.is_empty() {
        return Err("empty".into());
    }
    let v: Value = serde_json::from_str(t).map_err(|_| "invalid json".to_string())?;
    let p = protocol();
    if p.helper_to_host.is_valid(&v) {
        Ok(v)
    } else {
        // ADR 0083: where the line fails, never what it says (an `asr` line carries transcript text).
        let at = p
            .helper_to_host
            .iter_errors(&v)
            .next()
            .map(|e| format!(" at /{}", e.instance_path().to_string().trim_start_matches('/')))
            .unwrap_or_default();
        Err(format!(
            "type {} does not match the protocol{at}",
            v.get("type").and_then(Value::as_str).map(short).unwrap_or("?")
        ))
    }
}

/// Validate and encode a host → helper message as one line (sorted keys, like the golden fixtures).
pub fn encode_host_message(msg: &Value) -> Result<String, String> {
    if !protocol().host_to_helper.is_valid(msg) {
        // ADR 0083: never the message itself (a summarize prompt carries the transcript, dictation.configure the
        // dictionary); this error is logged.
        return Err(format!(
            "invalid host→helper message: type {} ({} bytes)",
            msg.get("type").and_then(Value::as_str).map(short).unwrap_or("?"),
            msg.to_string().len()
        ));
    }
    let mut s = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    s.push('\n');
    Ok(s)
}

/// A protocol `type` for an error message: kept only if it is a short identifier.
pub(crate) fn short(t: &str) -> &str {
    if t.len() <= 40
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        t
    } else {
        "?"
    }
}

pub fn message_type(v: &Value) -> Option<&str> {
    v.get("type").and_then(Value::as_str)
}

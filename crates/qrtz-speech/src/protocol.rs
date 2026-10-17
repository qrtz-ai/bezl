//! The speech module protocol (ADR 0046 §2). Source of truth: packages/core/src/speech/protocol.ts, rendered to
//! contracts/speech-protocol.json, which is embedded here.
use jsonschema::Validator;
use serde_json::Value;
use std::sync::OnceLock;

pub const SPEECH_PROTOCOL_JSON: &str = include_str!("../../../contracts/speech-protocol.json");
pub const SPEECH_PROTOCOL_VERSION: u32 = 1;

pub struct SpeechProtocol {
    pub version: u32,
    pub limits: Value,
    pub languages: Vec<String>,
    pub speech_to_host_types: Vec<String>,
    pub host_to_speech_types: Vec<String>,
    speech_to_host: Validator,
    host_to_speech: Validator,
}

pub fn protocol() -> &'static SpeechProtocol {
    static P: OnceLock<SpeechProtocol> = OnceLock::new();
    P.get_or_init(|| {
        let raw: Value = serde_json::from_str(SPEECH_PROTOCOL_JSON).expect("speech-protocol.json");
        let strs = |k: &str| -> Vec<String> {
            raw[k]
                .as_array()
                .expect(k)
                .iter()
                .map(|t| t.as_str().unwrap().to_owned())
                .collect()
        };
        let version = raw["protocolVersion"].as_u64().expect("protocolVersion") as u32;
        assert_eq!(version, SPEECH_PROTOCOL_VERSION, "regenerate contracts");
        SpeechProtocol {
            version,
            limits: raw["limits"].clone(),
            languages: strs("languages"),
            speech_to_host_types: strs("speechToHostTypes"),
            host_to_speech_types: strs("hostToSpeechTypes"),
            speech_to_host: jsonschema::validator_for(&raw["speechToHost"]).expect("speechToHost"),
            host_to_speech: jsonschema::validator_for(&raw["hostToSpeech"]).expect("hostToSpeech"),
        }
    })
}

/// Schema check of one module line (no bounds/state checks; see `validate::OutputValidator`).
pub fn parse_speech_line(line: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(line.trim()).map_err(|_| "invalid json".to_string())?;
    let p = protocol();
    if p.speech_to_host.is_valid(&v) {
        Ok(v)
    } else {
        Err(p
            .speech_to_host
            .iter_errors(&v)
            .next()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "invalid".into()))
    }
}

pub fn encode_host_to_speech(msg: &Value) -> Result<String, String> {
    if !protocol().host_to_speech.is_valid(msg) {
        return Err(format!("invalid host→speech message: {msg}"));
    }
    let mut s = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    s.push('\n');
    Ok(s)
}

pub fn limit(name: &str) -> u64 {
    protocol().limits[name]
        .as_u64()
        .unwrap_or_else(|| panic!("limit {name}"))
}

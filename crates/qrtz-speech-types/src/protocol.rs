//! The speech module protocol (ADR 0046 §2). Source of truth: packages/core/src/speech/protocol.ts, rendered to
//! contracts/speech-protocol.json, which is embedded here.
use jsonschema::Validator;
use serde_json::Value;
use std::sync::OnceLock;

pub const SPEECH_PROTOCOL_JSON: &str = include_str!("../../../contracts/speech-protocol.json");
pub const SPEECH_PROTOCOL_VERSION: u32 = 1;
/// N75: at most this many model files are passed to a module (qrtz-speech `files` re-exports it).
pub const MAX_MODEL_FDS: usize = 32;

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
/// A message's `type` for an error: kept only if it is a short identifier.
fn short_type(v: &Value) -> &str {
    match v.get("type").and_then(Value::as_str) {
        Some(t)
            if t.len() <= 40
                && t.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) =>
        {
            t
        }
        _ => "?",
    }
}

pub fn parse_speech_line(line: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(line.trim()).map_err(|_| "invalid json".to_string())?;
    let p = protocol();
    if p.speech_to_host.is_valid(&v) {
        Ok(v)
    } else {
        // ADR 0083: where the line fails, never what it says (`words` carry transcript text); this is logged.
        let at = p
            .speech_to_host
            .iter_errors(&v)
            .next()
            .map(|e| format!(" at /{}", e.instance_path().to_string().trim_start_matches('/')))
            .unwrap_or_default();
        Err(format!("type {} does not match the protocol{at}", short_type(&v)))
    }
}

pub fn encode_host_to_speech(msg: &Value) -> Result<String, String> {
    if !protocol().host_to_speech.is_valid(msg) {
        return Err(format!("invalid host→speech message: type {}", short_type(msg)));
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

/// N78: the `start` message, with the language hint checked against the allowlist and fd numbers in the
/// inherited range (7..7+n).
pub fn start_message(
    spec: &crate::engine::SessionSpec,
    files: &[(String, i32, u64)],
) -> Result<Value, String> {
    if let Some(h) = &spec.language_hint {
        if !protocol().languages.iter().any(|l| l == h) {
            return Err(format!("language hint {h} not allowed"));
        }
    }
    if files.len() > MAX_MODEL_FDS {
        return Err("too many model files".into());
    }
    for (i, (_, fd, _)) in files.iter().enumerate() {
        if *fd != 7 + i as i32 {
            return Err("model fds must be 7..7+n in order".into());
        }
    }
    let mut v = serde_json::json!({
        "type": "start",
        "sessionId": spec.session_id,
        "kind": match spec.kind { crate::engine::SessionKind::Recording => "recording", crate::engine::SessionKind::Dictation => "dictation" },
        "channels": spec.channels.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
        "sampleRate": 16000,
        "speakerMode": match spec.speaker_mode { crate::engine::SpeakerMode::Fast => "fast", crate::engine::SpeakerMode::Accurate => "accurate" },
        "files": files.iter().map(|(p, fd, size)| serde_json::json!({"path": p, "fd": fd, "size": size})).collect::<Vec<_>>(),
    });
    if let Some(h) = &spec.language_hint {
        v["languageHint"] = h.clone().into();
    }
    encode_host_to_speech(&v)?;
    Ok(v)
}

/// N77: read one line, bounded **before** allocation: at most `max` bytes are buffered; a longer line is consumed
/// and reported as `Err(TooLong)` without growing the buffer. `Ok(None)` at EOF.
pub async fn read_bounded_line<R: tokio::io::AsyncBufRead + Unpin>(
    r: &mut R,
    max: usize,
) -> std::io::Result<Option<Result<String, LineTooLong>>> {
    use tokio::io::AsyncBufReadExt;
    let mut out: Vec<u8> = Vec::new();
    let mut too_long = false;
    loop {
        let buf = r.fill_buf().await?;
        if buf.is_empty() {
            return Ok(if out.is_empty() && !too_long {
                None
            } else if too_long {
                Some(Err(LineTooLong))
            } else {
                Some(Ok(String::from_utf8_lossy(&out).into_owned()))
            });
        }
        let (chunk, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (&buf[..i], Some(i + 1)),
            None => (buf, None),
        };
        if !too_long {
            if out.len() + chunk.len() > max {
                too_long = true;
                out.clear();
                out.shrink_to_fit();
            } else {
                out.extend_from_slice(chunk);
            }
        }
        let n = done.unwrap_or(buf.len());
        r.consume(n);
        if done.is_some() {
            return Ok(Some(if too_long {
                Err(LineTooLong)
            } else {
                Ok(String::from_utf8_lossy(&out).into_owned())
            }));
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct LineTooLong;

//! The IPC contract, generated from zod by apps/desktop/src/shared/contract-json.test.ts.
use jsonschema::Validator;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

pub const CONTRACT_JSON: &str = include_str!("../../../contracts/ipc-contract.json");

pub enum RequestSpec {
    /// `z.void()`: the payload must be absent or null.
    Void,
    Schema {
        optional: bool,
        validator: Validator,
    },
}

pub struct ChannelDef {
    pub name: String,
    pub version: u64,
    pub capability: String,
    pub request: RequestSpec,
    pub response: Validator,
}

pub struct EventDef {
    pub name: String,
    pub capability: String,
    pub payload: Validator,
    /// High-rate: delivered over a per-window `Channel` stream, never the broadcast event (ADR 0095).
    pub stream: bool,
}

pub struct Contract {
    pub protocol_version: u64,
    pub error_codes: Vec<String>,
    pub channels: BTreeMap<String, ChannelDef>,
    pub events: BTreeMap<String, EventDef>,
    /// sub app id → capabilities.
    pub manifests: BTreeMap<String, BTreeSet<String>>,
    pub config_defaults: Value,
    pub config_schema: Validator,
    pub envelope: Validator,
    pub result: Validator,
    pub raw: Value,
}

fn compile(schema: &Value, what: &str) -> Validator {
    jsonschema::validator_for(schema)
        .unwrap_or_else(|e| panic!("contract schema for {what} does not compile: {e}"))
}

impl Contract {
    #[allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "embedded at build time; a bad file is a build defect caught by its unit tests"
    )]
    pub fn parse(text: &str) -> Contract {
        let raw: Value = serde_json::from_str(text).expect("ipc-contract.json is valid JSON");
        let channels = raw["channels"]
            .as_object()
            .expect("channels")
            .iter()
            .map(|(name, d)| {
                let req = &d["request"];
                let request = if req["void"].as_bool() == Some(true) {
                    RequestSpec::Void
                } else {
                    RequestSpec::Schema {
                        optional: req["optional"].as_bool().unwrap_or(false),
                        validator: compile(&req["schema"], &format!("{name} request")),
                    }
                };
                (
                    name.clone(),
                    ChannelDef {
                        name: name.clone(),
                        version: d["version"].as_u64().expect("version"),
                        capability: d["capability"].as_str().expect("capability").to_owned(),
                        request,
                        response: compile(&d["response"], &format!("{name} response")),
                    },
                )
            })
            .collect();
        let events = raw["events"]
            .as_object()
            .expect("events")
            .iter()
            .map(|(name, d)| {
                (
                    name.clone(),
                    EventDef {
                        name: name.clone(),
                        capability: d["capability"].as_str().expect("capability").to_owned(),
                        payload: compile(&d["payload"], &format!("{name} event")),
                        stream: d["stream"].as_bool().unwrap_or(false),
                    },
                )
            })
            .collect();
        let manifests = raw["manifests"]
            .as_array()
            .expect("manifests")
            .iter()
            .map(|m| {
                (
                    m["id"].as_str().expect("id").to_owned(),
                    m["capabilities"]
                        .as_array()
                        .expect("capabilities")
                        .iter()
                        .map(|c| c.as_str().expect("cap").to_owned())
                        .collect(),
                )
            })
            .collect();
        Contract {
            protocol_version: raw["protocolVersion"].as_u64().expect("protocolVersion"),
            error_codes: raw["errorCodes"]
                .as_array()
                .expect("errorCodes")
                .iter()
                .map(|c| c.as_str().unwrap().to_owned())
                .collect(),
            channels,
            events,
            manifests,
            config_defaults: raw["config"]["defaults"].clone(),
            config_schema: compile(&raw["config"]["schema"], "config"),
            envelope: compile(&raw["envelope"], "envelope"),
            result: compile(&raw["result"], "result"),
            raw,
        }
    }

    pub fn has_capability(&self, sub_app: &str, capability: &str) -> bool {
        self.manifests
            .get(sub_app)
            .is_some_and(|caps| caps.contains(capability))
    }
}

/// The embedded contract, compiled once.
pub fn contract() -> &'static Contract {
    static C: OnceLock<Contract> = OnceLock::new();
    C.get_or_init(|| Contract::parse(CONTRACT_JSON))
}

/// Collects up to 10 validation issues as `{path, message}` (same shape as the TS router's details).
pub fn issues(v: &Validator, value: &Value) -> Vec<Value> {
    v.iter_errors(value)
        .take(10)
        .map(|e| {
            serde_json::json!({
                "path": e.instance_path().to_string().trim_start_matches('/').replace('/', "."),
                "message": e.to_string(),
            })
        })
        .collect()
}

/// Up to 10 failing instance paths (`a.b.0`), no values: what may go into a log (ADR 0083).
pub fn issue_paths(v: &Validator, value: &Value) -> Vec<String> {
    let mut at: Vec<String> = v
        .iter_errors(value)
        .take(10)
        .map(|e| {
            e.instance_path()
                .to_string()
                .trim_start_matches('/')
                .replace('/', ".")
        })
        .collect();
    at.dedup();
    at
}

/// Golden samples (packages/core/src/ipc/samples.ts → contracts/ipc-samples.json).
pub const SAMPLES_JSON: &str = include_str!("../../../contracts/ipc-samples.json");

#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "embedded at build time; a bad file is a build defect caught by its unit tests"
)]
pub fn samples() -> &'static Value {
    static S: OnceLock<Value> = OnceLock::new();
    S.get_or_init(|| serde_json::from_str(SAMPLES_JSON).expect("ipc-samples.json"))
}

/// What an empty backend answers on `channel`, if the samples define one.
pub fn idle_response(channel: &str) -> Option<Value> {
    samples()["channels"][channel].get("idle").cloned()
}

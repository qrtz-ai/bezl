//! Event fan-out rules, ported from apps/desktop/src/main/ipc.ts: a sub app receives an event only if
//! it subscribed from a given window *and* its manifest holds the event's capability. Payloads are
//! validated before they fan out (EventBus.emit in TS).
use crate::contract::{contract, issues};
use crate::error::{AppError, ErrorCode};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default, Debug)]
pub struct Subscriptions {
    /// target (webview label) → event → sub apps.
    by_target: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

impl Subscriptions {
    pub fn subscribe(&mut self, target: &str, sub_app: &str, event: &str) -> Result<(), AppError> {
        if sub_app.is_empty() || sub_app.len() > 64 || event.is_empty() || event.len() > 64 {
            return Err(AppError::new(ErrorCode::Validation, "Malformed subscription"));
        }
        let c = contract();
        let def = c
            .events
            .get(event)
            .ok_or_else(|| AppError::new(ErrorCode::UnknownChannel, format!("Unknown event \"{event}\"")))?;
        if !c.has_capability(sub_app, &def.capability) {
            return Err(AppError::new(
                ErrorCode::Forbidden,
                format!("\"{sub_app}\" may not subscribe to \"{event}\""),
            ));
        }
        self.by_target
            .entry(target.to_owned())
            .or_default()
            .entry(event.to_owned())
            .or_default()
            .insert(sub_app.to_owned());
        Ok(())
    }

    /// The webview went away (closed or reloaded): drop everything it subscribed to.
    pub fn remove_target(&mut self, target: &str) {
        self.by_target.remove(target);
    }

    /// Targets that should receive `event`, each with the sub apps allowed to see it.
    pub fn fanout(&self, event: &str) -> Vec<(String, Vec<String>)> {
        self.by_target
            .iter()
            .filter_map(|(t, evs)| {
                let subs = evs.get(event)?;
                (!subs.is_empty()).then(|| (t.clone(), subs.iter().cloned().collect()))
            })
            .collect()
    }
}

/// Validate an outgoing event payload against the contract.
/// Where an event payload fails its schema (paths only, no values; for logs, ADR 0083).
pub fn validate_event_paths(event: &str, payload: &Value) -> Vec<String> {
    contract()
        .events
        .get(event)
        .map(|d| crate::contract::issue_paths(&d.payload, payload))
        .unwrap_or_default()
}

pub fn validate_event(event: &str, payload: &Value) -> Result<(), AppError> {
    let def = contract()
        .events
        .get(event)
        .ok_or_else(|| AppError::new(ErrorCode::UnknownChannel, format!("Unknown event \"{event}\"")))?;
    if def.payload.is_valid(payload) {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorCode::Internal,
            format!("Invalid payload for event \"{event}\""),
        )
        .details(Value::Array(issues(&def.payload, payload))))
    }
}

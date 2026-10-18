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
        if allowed(sub_app, event)? {
            return Err(AppError::new(
                ErrorCode::Validation,
                format!("\"{event}\" is a stream: open it with qrtz_stream_open"),
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

/// The shared rule for both delivery paths: a well-formed request for a known event whose capability the sub
/// app's manifest holds. `Ok(true)` when the event is a stream (ADR 0095).
fn allowed(sub_app: &str, event: &str) -> Result<bool, AppError> {
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
    Ok(def.stream)
}

/// Whether `event` is delivered over streams rather than the broadcast event.
pub fn is_stream(event: &str) -> bool {
    contract().events.get(event).is_some_and(|d| d.stream)
}

/// High-rate events over per-window channels (ADR 0095). A window opens one stream per event, with the first sub
/// app that listens; other sub apps in that window join it. Every open and join passes the same capability rule as
/// `subscribe`. `C` is the transport's channel (`tauri::ipc::Channel` in the app, anything in tests); dropping a
/// stream drops its channel.
pub struct Streams<C> {
    next: u64,
    by_id: BTreeMap<u64, Stream<C>>,
}

struct Stream<C> {
    target: String,
    event: String,
    sub_apps: BTreeSet<String>,
    chan: C,
}

impl<C> Default for Streams<C> {
    fn default() -> Self {
        Self {
            next: 1,
            by_id: BTreeMap::new(),
        }
    }
}

impl<C> Streams<C> {
    /// A new stream for `event` in `target`, delivered on `chan`. A window holds at most one stream per event:
    /// opening again replaces (drops) the old one.
    pub fn open(&mut self, target: &str, sub_app: &str, event: &str, chan: C) -> Result<u64, AppError> {
        if !allowed(sub_app, event)? {
            return Err(AppError::new(
                ErrorCode::Validation,
                format!("\"{event}\" is not a stream: subscribe to it"),
            ));
        }
        self.by_id
            .retain(|_, s| !(s.target == target && s.event == event));
        let id = self.next;
        self.next += 1;
        self.by_id.insert(
            id,
            Stream {
                target: target.to_owned(),
                event: event.to_owned(),
                sub_apps: BTreeSet::from([sub_app.to_owned()]),
                chan,
            },
        );
        Ok(id)
    }

    /// Another sub app in the same window listens on stream `id`.
    pub fn join(&mut self, target: &str, id: u64, sub_app: &str) -> Result<(), AppError> {
        let s = self
            .by_id
            .get_mut(&id)
            .filter(|s| s.target == target)
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "No such stream"))?;
        allowed(sub_app, &s.event)?;
        s.sub_apps.insert(sub_app.to_owned());
        Ok(())
    }

    /// The window's last listener for the stream went away. Only the window that opened it can close it.
    pub fn close(&mut self, target: &str, id: u64) -> bool {
        if self.by_id.get(&id).is_some_and(|s| s.target == target) {
            self.by_id.remove(&id);
            true
        } else {
            false
        }
    }

    /// The webview went away (closed or reloaded): drop its streams and their channels. Returns how many.
    pub fn remove_target(&mut self, target: &str) -> usize {
        let before = self.by_id.len();
        self.by_id.retain(|_, s| s.target != target);
        before - self.by_id.len()
    }

    /// A send failed (the page is gone): drop that stream.
    pub fn drop_id(&mut self, id: u64) {
        self.by_id.remove(&id);
    }

    /// `(id, channel)` for every stream of `event`.
    pub fn fanout<'a>(&'a self, event: &'a str) -> impl Iterator<Item = (u64, &'a C)> + 'a {
        self.by_id
            .iter()
            .filter(move |(_, s)| s.event == event)
            .map(|(id, s)| (*id, &s.chan))
    }

    /// Open streams (all windows).
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Sub apps allowed on stream `id` (tests, diagnostics).
    pub fn sub_apps(&self, id: u64) -> Vec<String> {
        self.by_id
            .get(&id)
            .map(|s| s.sub_apps.iter().cloned().collect())
            .unwrap_or_default()
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

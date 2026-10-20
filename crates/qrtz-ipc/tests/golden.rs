#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Golden IPC envelopes: every channel's sample request goes through the Rust router (envelope, capability,
//! request schema) and its sample response passes the Rust response schema; every event sample validates.
use async_trait::async_trait;
use qrtz_ipc::*;
use serde_json::{json, Value};

struct Golden;
#[async_trait]
impl Handler for Golden {
    async fn handle(&self, channel: &str, _p: Value, _c: &CallContext) -> Result<Value, AppError> {
        Ok(samples()["channels"][channel]["response"].clone())
    }
}

#[tokio::test]
async fn every_channel_round_trips_through_the_router() {
    let c = contract();
    let s = samples();
    let r = Router::new(Golden);
    assert_eq!(s["channels"].as_object().unwrap().len(), c.channels.len());
    for (name, def) in &c.channels {
        let sample = &s["channels"][name];
        let sub = c
            .manifests
            .iter()
            .find(|(_, caps)| caps.contains(&def.capability))
            .map(|(id, _)| id.clone())
            .unwrap();
        let env = json!({"v":1,"id":"g","channel":name,"version":def.version,"subApp":sub,"payload":sample["request"]});
        let res = r.dispatch(env).await;
        assert_eq!(res["ok"], true, "{name}: {res}");
        assert_eq!(res["data"], sample["response"], "{name}");
        if let Some(idle) = sample.get("idle") {
            assert!(def.response.is_valid(idle), "{name} idle");
        }
    }
}

#[test]
fn every_event_sample_validates() {
    let s = samples();
    for name in contract().events.keys() {
        validate_event(name, &s["events"][name]).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}

#[test]
fn ipc_result_shapes_validate() {
    let c = contract();
    assert!(c.result.is_valid(&json!({"ok": true, "id": "x", "data": 1})));
    let err = AppError::new(ErrorCode::NotFound, "nope").to_json();
    assert!(c.result.is_valid(&json!({"ok": false, "id": "x", "error": err})));
}

//! Port of packages/core/src/ipc/router.test.ts and apps/desktop/src/shared/ipc-capabilities.test.ts
//! against the real generated contract and manifests.
use async_trait::async_trait;
use qrtz_ipc::*;
use serde_json::{json, Value};

struct Echo;
#[async_trait]
impl Handler for Echo {
    async fn handle(&self, channel: &str, payload: Value, _ctx: &CallContext) -> Result<Value, AppError> {
        match channel {
            "app:get-info" => Ok(info()),
            "config:set" => Ok(json!({})),
            "bad:output" => unreachable!(),
            _ => {
                let _ = payload;
                Err(no_handler(channel))
            }
        }
    }
}
fn info() -> Value {
    json!({"name": "qrtz", "version": "1", "platform": "darwin", "arch": "arm64", "electron": "tauri 2"})
}
fn base() -> Value {
    json!({"v": 1, "id": "r1", "channel": "app:get-info", "version": 1, "subApp": "shell"})
}
fn with(mut v: Value, k: &str, val: Value) -> Value {
    v[k] = val;
    v
}
async fn code(r: &Router<Echo>, env: Value) -> String {
    let res = r.dispatch(env).await;
    assert_eq!(res["ok"], false, "{res}");
    res["error"]["code"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn dispatches_valid_calls_with_absent_payload() {
    let r = Router::new(Echo);
    assert_eq!(
        r.dispatch(base()).await,
        json!({"ok": true, "id": "r1", "data": info()})
    );
}

#[tokio::test]
async fn rejects_like_the_ts_router() {
    let r = Router::new(Echo);
    assert_eq!(code(&r, with(base(), "v", json!(2))).await, "E_VALIDATION");
    assert_eq!(code(&r, json!("garbage")).await, "E_VALIDATION");
    assert_eq!(
        code(&r, with(base(), "channel", json!("nope"))).await,
        "E_UNKNOWN_CHANNEL"
    );
    assert_eq!(code(&r, with(base(), "version", json!(9))).await, "E_VERSION");
    assert_eq!(
        code(&r, with(base(), "subApp", json!("ghost"))).await,
        "E_FORBIDDEN"
    );
    // shell lacks db.read
    assert_eq!(
        code(&r, with(base(), "channel", json!("db:health"))).await,
        "E_FORBIDDEN"
    );
    // void channel with a payload
    assert_eq!(
        code(&r, with(base(), "payload", json!({"x": 1}))).await,
        "E_VALIDATION"
    );
    // garbage id still answers with "unknown"
    let res = r.dispatch(json!({"nope": true})).await;
    assert_eq!(res["id"], "unknown");
}

#[tokio::test]
async fn validates_request_payloads_with_issue_details() {
    let r = Router::new(Echo);
    let env = json!({"v":1,"id":"r2","channel":"config:set","version":1,"subApp":"settings",
        "payload":{"patch": 5}});
    let res = r.dispatch(env.clone()).await;
    assert_eq!(res["error"]["code"], "E_VALIDATION");
    assert!(res["error"]["details"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["path"] == "patch"));
    let mut ok = env;
    ok["payload"]["patch"] = json!({});
    assert_eq!(r.dispatch(ok).await["ok"], true);
}

#[tokio::test]
async fn optional_requests_accept_absent_payloads() {
    struct Start;
    #[async_trait]
    impl Handler for Start {
        async fn handle(&self, _c: &str, p: Value, _x: &CallContext) -> Result<Value, AppError> {
            assert!(p.is_null());
            Err(AppError::new(ErrorCode::NotFound, "probe"))
        }
    }
    let r = Router::new(Start);
    let env = json!({"v":1,"id":"r3","channel":"note:live-transcript","version":1,"subApp":"note"});
    assert_eq!(r.dispatch(env).await["error"]["code"], "E_NOT_FOUND");
}

#[tokio::test]
async fn rejects_invalid_handler_output_and_hides_internals() {
    struct Bad;
    #[async_trait]
    impl Handler for Bad {
        async fn handle(&self, _c: &str, _p: Value, _x: &CallContext) -> Result<Value, AppError> {
            Ok(json!({"nope": 1}))
        }
    }
    let res = Router::new(Bad).dispatch(base()).await;
    assert_eq!(res["error"]["code"], "E_INTERNAL");
    assert_eq!(
        res["error"]["message"],
        "Handler for \"app:get-info\" returned invalid data"
    );
    assert_eq!(res["error"]["retryable"], false);
}

#[tokio::test]
async fn unavailable_handlers_are_retryable() {
    let r = Router::new(Echo);
    let env = json!({"v":1,"id":"r4","channel":"helper:status","version":1,"subApp":"shell"});
    let res = r.dispatch(env).await;
    assert_eq!(res["error"]["code"], "E_UNAVAILABLE");
    assert_eq!(res["error"]["retryable"], true);
}

/// Same matrix as ipc-capabilities.test.ts, on the real manifests.
#[tokio::test]
async fn sub_app_capability_matrix() {
    struct Any;
    #[async_trait]
    impl Handler for Any {
        async fn handle(&self, _c: &str, _p: Value, _x: &CallContext) -> Result<Value, AppError> {
            Ok(Value::Null)
        }
    }
    let r = Router::new(Any).validate_responses(false);
    let cases = [
        ("note", "note:list", json!({"query": "x"}), true),
        ("note", "note:start", json!({}), true),
        (
            "note",
            "permissions:request",
            json!({"permission": "microphone"}),
            true,
        ),
        ("note", "config:set", json!({"patch": {}}), false),
        ("gallery", "note:list", json!({"query": ""}), false),
        ("settings", "config:set", json!({"patch": {}}), true),
        ("settings", "note:delete", json!({"id": "m"}), false),
    ];
    for (sub, ch, payload, allowed) in cases {
        let env = json!({"v":1,"id":"m","channel":ch,"version":1,"subApp":sub,"payload":payload});
        let res = r.dispatch(env).await;
        if allowed {
            assert_eq!(res["ok"], true, "{sub} {ch}: {res}");
        } else {
            assert_eq!(res["error"]["code"], "E_FORBIDDEN", "{sub} {ch}: {res}");
        }
    }
    let env = json!({"v":1,"id":"m","channel":"note:get","version":1,"subApp":"note","payload":{"id":"x".repeat(100)}});
    assert_eq!(r.dispatch(env).await["error"]["code"], "E_VALIDATION");
}

#[test]
fn error_codes_match_the_contract() {
    let ours: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(ours, contract().error_codes);
}

#[test]
fn contract_is_complete() {
    let c = contract();
    assert!(c.channels.len() >= 30, "{}", c.channels.len());
    assert!(c.events.contains_key("note:transcript"));
    assert!(c.config_schema.is_valid(&c.config_defaults));
    for d in c.channels.values() {
        assert!(
            c.manifests.values().any(|caps| caps.contains(&d.capability)),
            "{}",
            d.name
        );
    }
}

#[test]
fn subscriptions_follow_capabilities_and_targets() {
    // note:pipeline: a low-rate event on the broadcast path (the high-rate ones are streams, tests/streams.rs).
    let mut s = Subscriptions::default();
    s.subscribe("main", "note", "note:pipeline").unwrap();
    s.subscribe("main", "shell", "note:pipeline").unwrap();
    s.subscribe("hud", "shell", "note:pipeline").unwrap();
    assert_eq!(
        s.subscribe("main", "gallery", "note:pipeline").unwrap_err().code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        s.subscribe("main", "note", "nope").unwrap_err().code,
        ErrorCode::UnknownChannel
    );
    let mut f = s.fanout("note:pipeline");
    f.sort();
    assert_eq!(
        f,
        vec![
            ("hud".into(), vec!["shell".into()]),
            ("main".into(), vec!["note".into(), "shell".into()])
        ]
    );
    s.remove_target("main");
    assert_eq!(s.fanout("note:pipeline").len(), 1);
    assert!(s.fanout("note:changed").is_empty());
}

#[test]
fn event_payloads_are_validated() {
    assert!(validate_event("note:changed", &json!({"id": "m1"})).is_ok());
    assert_eq!(
        validate_event("note:changed", &json!({})).unwrap_err().code,
        ErrorCode::Internal
    );
    assert!(validate_event("helper:status", &json!({"state": "running", "restarts": 0})).is_ok());
    assert!(validate_event("helper:status", &json!({"state": "weird", "restarts": 0})).is_err());
}

/// ADR 0217 (H1, Bozniak re-review): calendar.read is nano's alone. talk, ctrl and the shell are refused the
/// calendar's status, upcoming events and refresh, and its calendar:changed event; the shell's onboarding gets only
/// calendar:access (os.permissions), and nano's view gets everything.
#[tokio::test]
async fn only_nano_reads_the_calendar() {
    struct Any;
    #[async_trait]
    impl Handler for Any {
        async fn handle(&self, _c: &str, _p: Value, _x: &CallContext) -> Result<Value, AppError> {
            Ok(Value::Null)
        }
    }
    let r = Router::new(Any).validate_responses(false);
    let call = |sub: &str, ch: &str| json!({"v":1,"id":"c","channel":ch,"version":1,"subApp":sub});
    let reads = ["calendar:status", "calendar:upcoming", "calendar:refresh"];
    for sub in ["talk", "ctrl", "shell"] {
        for ch in reads {
            let res = r.dispatch(call(sub, ch)).await;
            assert_eq!(res["error"]["code"], "E_FORBIDDEN", "{sub} {ch}: {res}");
        }
        let mut s = Subscriptions::default();
        assert_eq!(
            s.subscribe("main", sub, "calendar:changed").unwrap_err().code,
            ErrorCode::Forbidden,
            "{sub} calendar:changed"
        );
    }
    for ch in reads {
        assert_eq!(r.dispatch(call("note", ch)).await["ok"], true, "note {ch}");
    }
    Subscriptions::default()
        .subscribe("main", "note", "calendar:changed")
        .unwrap();
    assert_eq!(r.dispatch(call("shell", "calendar:access")).await["ok"], true);
    assert!(!contract().manifests["shell"].contains("calendar.read"));
    let holders: Vec<&String> = contract()
        .manifests
        .iter()
        .filter(|(_, caps)| caps.contains("calendar.read"))
        .map(|(k, _)| k)
        .collect();
    assert_eq!(holders, ["note"], "calendar.read is nano's view's alone");
}

/// Bozniak vault 6: doc.read (nano's note files) belongs to nano alone, and only nano can subscribe to
/// `doc:changed`.
#[test]
fn doc_read_is_pinned_to_note_alone() {
    let c = contract();
    let holders: Vec<&String> = c
        .manifests
        .iter()
        .filter(|(_, caps)| caps.contains("doc.read"))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(holders, ["note"], "only nano holds doc.read");
    for ch in ["doc:list", "doc:get"] {
        assert_eq!(c.channels[ch].capability, "doc.read", "{ch}");
    }
    let mut subs = Subscriptions::default();
    assert!(subs.subscribe("w", "note", "doc:changed").is_ok());
    for sub in c.manifests.keys().filter(|k| *k != "note") {
        assert!(
            subs.subscribe("w", sub, "doc:changed").is_err(),
            "{sub} subscribed to doc:changed"
        );
    }
}

/// ADR 0228: Agent Dash's view and controls are dash's view's alone. The shell and every other module are
/// refused every dash:* channel; dash's view gets them all.
#[tokio::test]
async fn only_dash_reads_and_manages_the_dashboard() {
    struct Any;
    #[async_trait]
    impl Handler for Any {
        async fn handle(&self, _c: &str, _p: Value, _x: &CallContext) -> Result<Value, AppError> {
            Ok(Value::Null)
        }
    }
    let r = Router::new(Any).validate_responses(false);
    let channels: Vec<&String> = contract()
        .channels
        .keys()
        .filter(|c| c.starts_with("dash:"))
        .collect();
    assert_eq!(channels.len(), 8);
    let others: Vec<&String> = contract().manifests.keys().filter(|k| *k != "dash").collect();
    assert!(others.len() >= 7);
    for ch in &channels {
        let payload = match ch.as_str() {
            "dash:review-agent" => json!({"agentId": "ag_1", "approve": true}),
            "dash:delete" => json!({"scope": "dashboard"}),
            "dash:connect" => json!({}),
            "dash:open-link" => json!({"url": "https://qrtz.ai"}),
            _ => Value::Null,
        };
        let call =
            |sub: &str| json!({"v":1,"id":"c","channel":ch,"version":1,"subApp":sub,"payload":payload});
        for sub in &others {
            let res = r.dispatch(call(sub)).await;
            assert_eq!(res["error"]["code"], "E_FORBIDDEN", "{sub} {ch}: {res}");
        }
        assert_eq!(r.dispatch(call("dash")).await["ok"], true, "dash {ch}");
    }
    for cap in ["dash.read", "dash.manage"] {
        let holders: Vec<&String> = contract()
            .manifests
            .iter()
            .filter(|(_, caps)| caps.contains(cap))
            .map(|(k, _)| k)
            .collect();
        assert_eq!(holders, ["dash"], "{cap} is dash's alone");
    }
}

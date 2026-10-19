//! ADR 0231 amendment 1: the helper's egress pin, its stdio protocol and where the owner key goes.
use qrtz_dash_net::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const KEY: &str =
    "ok_0123456789abcdef0123456789abcdef_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

type Seen = Arc<Mutex<Vec<(Method, String, String, Option<Value>)>>>;

/// Records every request instead of sending it, and answers `status`/`body`.
struct Fake(Seen, u16, Value);

#[async_trait::async_trait]
impl Transport for Fake {
    async fn send(
        &self,
        m: Method,
        url: &str,
        bearer: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), qrtz_ipc::AppError> {
        self.0.lock().unwrap().push((m, url.into(), bearer.into(), body));
        Ok((self.1, self.2.clone()))
    }
}

fn helper(status: u16, body: Value, key: Option<&str>) -> (Helper, Seen) {
    let seen = Seen::default();
    let store = MemoryStore::default();
    if let Some(k) = key {
        store.save(k).unwrap();
    }
    (
        Helper {
            transport: Box::new(Fake(seen.clone(), status, body)),
            store: Box::new(store),
        },
        seen,
    )
}

#[test]
fn egress_is_pinned_to_https_dash_qrtz_ai() {
    assert_eq!(ORIGIN, qrtz_dash::DASH_ORIGIN);
    assert!(egress_ok("https://dash.qrtz.ai/v1/owner/view"));
    for u in [
        "http://dash.qrtz.ai/v1/owner/view",
        "https://dash.qrtz.ai.evil.com/v1/owner/view",
        "https://evil.com/v1/owner/view",
        "https://dash.qrtz.ai:8443/v1/owner/view",
        "https://dash.qrtz.ai@evil.com/v1",
        "https://user@dash.qrtz.ai/v1",
        "https://dash.qrtz.ai/v1/../x",
        "https://dash.qrtz.ai/v1#x",
        "https://dash.qrtz.ai",
        "https://DASH.qrtz.ai/v1",
        "https://127.0.0.1/v1",
        "ws://dash.qrtz.ai/v1",
        "-https://dash.qrtz.ai/v1",
        "https://dash.qrtz.ai/v1 x",
    ] {
        assert!(!egress_ok(u), "{u}");
    }
}

#[tokio::test]
async fn every_channel_goes_to_its_owner_route_on_the_pinned_origin_with_the_key_in_the_header() {
    let (h, seen) = helper(200, json!({"ok": true, "readme_version": "2.1"}), Some(KEY));
    let mut v = h.call("dash:view", &Value::Null).await.unwrap();
    assert_eq!(v["connected"], true);
    v.as_object_mut().unwrap().remove("connected");
    assert_eq!(v, json!({"ok": true}));
    h.call("dash:issue-code", &Value::Null).await.unwrap();
    h.call("dash:rotate-codes", &Value::Null).await.unwrap();
    h.call("dash:review-agent", &json!({"agentId": "ag_1", "approve": true}))
        .await
        .unwrap();
    h.call(
        "dash:delete",
        &json!({"scope": "widget", "agentId": "ag_1", "key": "k-1"}),
    )
    .await
    .unwrap();
    h.call("dash:delete", &json!({"scope": "agent", "agentId": "ag_1"}))
        .await
        .unwrap();
    h.call("dash:delete", &json!({"scope": "dashboard"}))
        .await
        .unwrap();
    // Path injection is refused before any request.
    for p in [
        json!({"scope": "agent", "agentId": "../x"}),
        json!({"scope": "agent", "agentId": "a/b"}),
        json!({"scope": "widget", "agentId": "a", "key": "@evil.com"}),
        json!({"scope": "nope"}),
    ] {
        assert!(h.call("dash:delete", &p).await.is_err(), "{p}");
    }
    let seen = seen.lock().unwrap().clone();
    let got: Vec<(Method, &str)> = seen.iter().map(|(m, u, _, _)| (*m, u.as_str())).collect();
    assert_eq!(
        got,
        vec![
            (Method::Get, "https://dash.qrtz.ai/v1/owner/view"),
            (Method::Post, "https://dash.qrtz.ai/v1/owner/codes"),
            (Method::Post, "https://dash.qrtz.ai/v1/owner/codes/rotate"),
            (Method::Post, "https://dash.qrtz.ai/v1/owner/agents/ag_1/review"),
            (Method::Delete, "https://dash.qrtz.ai/v1/owner/widgets/ag_1/k-1"),
            (Method::Delete, "https://dash.qrtz.ai/v1/owner/agents/ag_1"),
            (Method::Delete, "https://dash.qrtz.ai/v1/owner/dashboard"),
        ]
    );
    for (_, u, bearer, _) in &seen {
        assert!(egress_ok(u));
        assert!(!u.contains(&KEY[36..]), "key in the url");
        assert_eq!(bearer, KEY);
    }
    assert_eq!(seen[3].3, Some(json!({"approve": true})));
}

#[tokio::test]
async fn connect_keeps_the_owner_key_in_the_helper_and_never_returns_it() {
    let (h, seen) = helper(201, json!({"dashboard_id": "d", "owner_key": KEY}), None);
    let r = h
        .call("dash:connect", &json!({"setupToken": " setup "}))
        .await
        .unwrap();
    assert_eq!(r, json!({"connected": true, "origin": "https://dash.qrtz.ai"}));
    assert!(!r.to_string().contains(&KEY[36..]));
    assert_eq!(h.store.load().as_deref(), Some(KEY));
    {
        let s = seen.lock().unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].1, "https://dash.qrtz.ai/v1/owner/setup");
        assert_eq!(s[0].2, "setup");
    }
    // Reconnect without a token reuses the stored key, with no request.
    h.call("dash:connect", &json!({})).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
    // A server body that echoes owner_key on another route is stripped too.
    let (h2, _) = helper(200, json!({"owner_key": KEY, "x": 1}), Some(KEY));
    let v = h2.call("dash:view", &Value::Null).await.unwrap();
    assert!(!v.to_string().contains(&KEY[36..]));
    // Disconnect forgets it.
    h.call("dash:disconnect", &Value::Null).await.unwrap();
    assert!(h.store.load().is_none());
    assert!(h.call("dash:view", &Value::Null).await.is_err());
}

#[tokio::test]
async fn errors_map_and_a_bad_owner_key_is_not_saved() {
    let (h, _) = helper(401, json!({"error": "owner key invalid"}), Some(KEY));
    assert_eq!(
        h.call("dash:view", &Value::Null).await.unwrap_err().code,
        qrtz_ipc::ErrorCode::Forbidden
    );
    let (h, _) = helper(201, json!({"owner_key": "ok_short"}), None);
    assert!(h.call("dash:connect", &json!({"setupToken": "t"})).await.is_err());
    assert!(h.store.load().is_none());
    let (h, seen) = helper(200, json!({}), None);
    assert!(
        h.call("dash:view", &Value::Null).await.is_err(),
        "no key: no request"
    );
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_stdio_protocol_checks_channel_and_capability_against_the_contract() {
    let contract: Value = serde_json::from_str(qrtz_ipc::contract::CONTRACT_JSON).unwrap();
    for (c, cap) in CHANNELS {
        assert_eq!(contract["channels"][c]["capability"], *cap, "{c}");
    }
    let (h, seen) = helper(200, json!({"ok": true}), Some(KEY));
    let ok = h
        .handle_line(r#"{"id":1,"channel":"dash:rotate-codes","capability":"dash.manage"}"#)
        .await;
    assert_eq!(ok, json!({"id": 1, "ok": {"ok": true}}));
    for line in [
        r#"{"id":2,"channel":"dash:rotate-codes","capability":"dash.read"}"#.to_owned(),
        r#"{"id":3,"channel":"vault:read","capability":"dash.manage"}"#.to_owned(),
        r#"{"id":4,"channel":"dash:open-link","capability":"dash.read"}"#.to_owned(),
        "not json".to_owned(),
        format!(
            r#"{{"id":5,"channel":"dash:view","capability":"dash.read","params":"{}"}}"#,
            "x".repeat(MAX_LINE)
        ),
    ] {
        let r = h.handle_line(&line).await;
        assert!(r.get("error").is_some(), "{line:.80}: {r}");
    }
    assert_eq!(seen.lock().unwrap().len(), 1, "refused lines send nothing");
}

#[tokio::test]
async fn stdin_lines_are_capped_and_an_overlong_line_is_drained_to_its_newline() {
    let mut data = vec![b'a'; MAX_LINE * 5];
    data.extend_from_slice(b"\n{\"id\":1}\nlast");
    let mut r = tokio::io::BufReader::with_capacity(1024, &data[..]);
    assert_eq!(read_line_capped(&mut r, MAX_LINE).await.unwrap(), Line::TooLong);
    assert_eq!(
        read_line_capped(&mut r, MAX_LINE).await.unwrap(),
        Line::Ok(b"{\"id\":1}".to_vec())
    );
    assert_eq!(
        read_line_capped(&mut r, MAX_LINE).await.unwrap(),
        Line::Ok(b"last".to_vec())
    );
    assert_eq!(read_line_capped(&mut r, MAX_LINE).await.unwrap(), Line::Eof);
    // Exactly MAX_LINE is fine.
    let mut ok = vec![b'b'; MAX_LINE];
    ok.push(b'\n');
    let mut r = &ok[..];
    assert_eq!(
        read_line_capped(&mut r, MAX_LINE).await.unwrap(),
        Line::Ok(vec![b'b'; MAX_LINE])
    );
    assert_eq!(line_too_long()["error"]["code"], "E_VALIDATION");
}

#[test]
fn the_release_gate_marker_is_in_the_binary_and_in_verify_bundle() {
    let bin = std::fs::read(env!("CARGO_BIN_EXE_qrtz-dash-net")).unwrap();
    let m = BUNDLE_MARKER.as_bytes();
    assert!(
        bin.windows(m.len()).any(|w| w == m),
        "marker not compiled into the helper"
    );
    let sh = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/release/verify-bundle.sh"
    ))
    .unwrap();
    assert!(
        sh.contains(BUNDLE_MARKER),
        "verify-bundle.sh greps a different marker"
    );
}

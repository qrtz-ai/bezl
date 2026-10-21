//! The agent API end to end (ADR 0228): enrollment, approval, writes, batch, tombstones, heartbeat, retention.
use qrtz_dash::*;
use serde_json::{json, Value};

const T0: u64 = 1_800_000_000_000;

fn dash() -> Dash {
    Dash::new(Pepper::from_bytes([7; 32]))
}

fn enroll(d: &mut Dash, name: &str) -> (String, String) {
    let code = d.issue_code(T0).unwrap();
    let r = d.handle(
        &Request::new("POST", "/v1/enroll").json(json!({"code": code, "name": name})),
        T0,
    );
    assert_eq!(r.status, 201, "{:?}", r.body);
    (
        r.body["agent_id"].as_str().unwrap().into(),
        r.body["agent_token"].as_str().unwrap().into(),
    )
}

fn approved(d: &mut Dash) -> (String, String) {
    let (id, tok) = enroll(d, "grok bot");
    assert!(d.approve(&id, T0));
    (id, tok)
}

fn post(d: &mut Dash, tok: &str, path: &str, body: Value) -> Response {
    d.handle(&Request::new("POST", path).bearer(tok).json(body), T0)
}

#[test]
fn a_code_is_single_use_expires_and_never_writes() {
    let mut d = dash();
    let code = d.issue_code(T0).unwrap();
    let body = json!({"code": code, "name": "grok bot", "description": "jared's assistant"});
    let r = d.handle(&Request::new("POST", "/v1/enroll").json(body.clone()), T0);
    assert_eq!(r.status, 201);
    assert_eq!(r.body["status"], "pending");
    assert_eq!(r.body["readme_version"], README_VERSION);
    assert!(r.body["agent_token"].as_str().unwrap().starts_with("at_"));
    assert_eq!(r.body["fingerprint"].as_str().unwrap().len(), 9);
    // used
    assert_eq!(
        d.handle(&Request::new("POST", "/v1/enroll").json(body), T0)
            .status,
        401
    );
    // expired
    let late = d.issue_code(T0).unwrap();
    let r = d.handle(
        &Request::new("POST", "/v1/enroll").json(json!({"code": late, "name": "other"})),
        T0 + ENROLL_TTL_MS + 1,
    );
    assert_eq!(r.status, 401);
    // a code is never a bearer
    let c = d.issue_code(T0).unwrap();
    assert_eq!(
        post(&mut d, &c, "/v1/heartbeat", json!({"status": "idle"})).status,
        401
    );
    // rotation drops outstanding codes
    let c = d.issue_code(T0).unwrap();
    d.rotate_codes(T0);
    assert_eq!(
        d.handle(
            &Request::new("POST", "/v1/enroll").json(json!({"code": c, "name": "x2"})),
            T0
        )
        .status,
        401
    );
}

#[test]
fn enrolling_again_makes_a_new_pending_agent_and_names_are_policed() {
    let mut d = dash();
    let (a, _) = enroll(&mut d, "worker one");
    let code = d.issue_code(T0).unwrap();
    // same name (folded) is a conflict while the first one stands
    let r = d.handle(
        &Request::new("POST", "/v1/enroll").json(json!({"code": code, "name": "W0rker One"})),
        T0,
    );
    assert_eq!(r.status, 409);
    for n in ["System", "ADMIN", "qrtz", "Adm1n", ""] {
        let code = d.issue_code(T0).unwrap();
        let r = d.handle(
            &Request::new("POST", "/v1/enroll").json(json!({"code": code, "name": n})),
            T0,
        );
        assert_eq!(r.status, 422, "{n}");
    }
    let (b, _) = enroll(&mut d, "worker two");
    assert_ne!(a, b);
}

#[test]
fn pending_rejected_and_bad_tokens() {
    let mut d = dash();
    let (id, tok) = enroll(&mut d, "grok bot");
    let r = post(&mut d, &tok, "/v1/heartbeat", json!({"status": "running"}));
    assert_eq!((r.status, r.body["error"].clone()), (403, json!("pending")));
    assert_eq!(
        post(&mut d, "at_nope", "/v1/heartbeat", json!({"status": "idle"})).body["action"],
        "stop"
    );
    // token in the url is ignored: still no auth
    let r = d.handle(
        &Request::new("POST", &format!("/v1/heartbeat?token={tok}")).json(json!({"status": "idle"})),
        T0,
    );
    assert_eq!(r.status, 401);
    assert!(d.reject(&id, T0));
    let r = post(&mut d, &tok, "/v1/heartbeat", json!({"status": "running"}));
    assert_eq!((r.status, r.body["action"].clone()), (401, json!("stop")));
}

#[test]
fn widgets_register_put_append_and_validate() {
    let mut d = dash();
    let (_, tok) = approved(&mut d);
    let r = post(
        &mut d,
        &tok,
        "/v1/widgets",
        json!({"key": "state-farm", "type": "status", "title": "State Farm CA policy"}),
    );
    assert_eq!(r.status, 201);
    let wid = r.body["widget_id"].clone();
    // same key + type is safe; other type is 409; unknown type 422; the old action block is gone
    assert_eq!(
        post(
            &mut d,
            &tok,
            "/v1/widgets",
            json!({"key": "state-farm", "type": "status"})
        )
        .body["widget_id"],
        wid
    );
    assert_eq!(
        post(
            &mut d,
            &tok,
            "/v1/widgets",
            json!({"key": "state-farm", "type": "log"})
        )
        .status,
        409
    );
    assert_eq!(
        post(&mut d, &tok, "/v1/widgets", json!({"key": "x", "type": "action"})).status,
        422
    );
    let put = |d: &mut Dash, k: &str, b: Value| {
        d.handle(
            &Request::new("PUT", &format!("/v1/widgets/{k}"))
                .bearer(&tok)
                .json(b),
            T0,
        )
    };
    assert_eq!(
        put(
            &mut d,
            "state-farm",
            json!({"status": "blocked", "text": "waiting"})
        )
        .status,
        200
    );
    assert_eq!(put(&mut d, "state-farm", json!({"status": "nope"})).status, 422);
    let r = put(&mut d, "ghost", json!({"status": "done"}));
    assert_eq!(
        (r.status, r.body["action"].clone()),
        (404, json!("register_own_widget"))
    );
    post(
        &mut d,
        &tok,
        "/v1/widgets",
        json!({"key": "activity", "type": "log"}),
    );
    assert_eq!(
        post(
            &mut d,
            &tok,
            "/v1/widgets/activity/events",
            json!({"line": "uploaded", "level": "info"})
        )
        .status,
        200
    );
    assert_eq!(
        post(
            &mut d,
            &tok,
            "/v1/widgets/state-farm/events",
            json!({"line": "x"})
        )
        .status,
        422
    );
    // body cap
    let big = json!({"line": "x".repeat(MAX_BODY_BYTES)});
    assert_eq!(post(&mut d, &tok, "/v1/widgets/activity/events", big).status, 413);
    let v = d.view(T0);
    let ws = &v["agents"][0]["widgets"];
    assert_eq!(ws.as_array().unwrap().len(), 2);
    assert!(!v.to_string().contains("at_"), "the view never carries a token");
}

#[test]
fn widgets_are_the_agents_own() {
    let mut d = dash();
    let (_, a) = approved(&mut d);
    let (bid, b) = enroll(&mut d, "other bot");
    d.approve(&bid, T0);
    post(&mut d, &a, "/v1/widgets", json!({"key": "k", "type": "stat"}));
    let r = d.handle(
        &Request::new("PUT", "/v1/widgets/k")
            .bearer(&b)
            .json(json!({"value": 1})),
        T0,
    );
    assert_eq!(r.status, 404, "b can't write a's widget");
}

#[test]
fn batch_caps_at_twenty_ops_and_reports_each() {
    let mut d = dash();
    let (_, tok) = approved(&mut d);
    post(&mut d, &tok, "/v1/widgets", json!({"key": "s", "type": "stat"}));
    post(&mut d, &tok, "/v1/widgets", json!({"key": "c", "type": "chart"}));
    let ops: Vec<Value> = (0..BATCH_MAX_OPS + 1)
        .map(|i| json!({"op": "append", "key": "c", "data": {"value": i}}))
        .collect();
    assert_eq!(post(&mut d, &tok, "/v1/batch", json!({ "ops": ops })).status, 422);
    let r = post(
        &mut d,
        &tok,
        "/v1/batch",
        json!({"ops": [
            {"op": "put", "key": "s", "data": {"value": 3}},
            {"op": "append", "key": "c", "data": {"value": 1}},
            {"op": "put", "key": "s", "data": {"value": "x"}},
            {"op": "delete", "key": "s"},
        ]}),
    );
    assert_eq!(r.status, 200);
    let st: Vec<u64> = r.body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["status"].as_u64().unwrap())
        .collect();
    assert_eq!(st, [200, 200, 422, 422]);
}

#[test]
fn deletes_are_tombstones_that_answer_410() {
    let mut d = dash();
    let (id, tok) = approved(&mut d);
    post(&mut d, &tok, "/v1/widgets", json!({"key": "k", "type": "stat"}));
    assert!(d.delete_widget(&id, "k", T0));
    let r = d.handle(
        &Request::new("PUT", "/v1/widgets/k")
            .bearer(&tok)
            .json(json!({"value": 1})),
        T0,
    );
    assert_eq!(
        (r.status, r.body["scope"].clone(), r.body["action"].clone()),
        (410, json!("widget"), json!("stop"))
    );
    // never re-registered
    assert_eq!(
        post(&mut d, &tok, "/v1/widgets", json!({"key": "k", "type": "stat"})).status,
        410
    );
    // batch op on it reports 410 too
    let r = post(
        &mut d,
        &tok,
        "/v1/batch",
        json!({"ops": [{"op": "put", "key": "k", "data": {"value": 1}}]}),
    );
    assert_eq!(r.body["results"][0]["status"], 410);
    assert!(d.delete_agent(&id, T0));
    let r = post(&mut d, &tok, "/v1/heartbeat", json!({"status": "idle"}));
    assert_eq!((r.status, r.body["scope"].clone()), (410, json!("agent")));
    let (_, t2) = approved(&mut d);
    d.delete_dashboard(T0);
    let r = post(&mut d, &t2, "/v1/heartbeat", json!({"status": "idle"}));
    assert_eq!((r.status, r.body["scope"].clone()), (410, json!("dashboard")));
    assert!(d.issue_code(T0).is_none());
}

#[test]
fn heartbeat_and_stale() {
    let mut d = dash();
    let (_, tok) = approved(&mut d);
    assert_eq!(
        post(&mut d, &tok, "/v1/heartbeat", json!({"status": "running"})).status,
        200
    );
    assert_eq!(
        post(&mut d, &tok, "/v1/heartbeat", json!({"status": "busy"})).status,
        422
    );
    let v = d.view(T0 + STALE_AFTER_MS);
    assert_eq!(
        (
            v["agents"][0]["stale"].clone(),
            v["agents"][0]["heartbeat"].clone()
        ),
        (json!(false), json!("running"))
    );
    assert_eq!(d.view(T0 + STALE_AFTER_MS + 1)["agents"][0]["stale"], true);
}

#[test]
fn history_older_than_retention_is_dropped_and_rings_cap() {
    let mut d = dash();
    let (_, tok) = approved(&mut d);
    post(&mut d, &tok, "/v1/widgets", json!({"key": "l", "type": "log"}));
    post(&mut d, &tok, "/v1/widgets/l/events", json!({"line": "old"}));
    let ev_at = |d: &mut Dash, line: &str, at: u64| {
        let r = d.handle(
            &Request::new("POST", "/v1/widgets/l/events")
                .bearer(&tok)
                .json(json!({ "line": line })),
            at,
        );
        assert_eq!(r.status, 200);
    };
    // A write half way keeps the agent and widget inside retention (QRT-307); only the old event ages out.
    ev_at(&mut d, "mid", T0 + RETENTION_DAYS * DAY_MS / 2);
    let later = T0 + RETENTION_DAYS * DAY_MS + 1;
    ev_at(&mut d, "new", later);
    let ev = d.view(later)["agents"][0]["widgets"][0]["events"].clone();
    assert_eq!(ev.as_array().unwrap().len(), 2);
    assert_eq!(ev[0]["data"]["line"], "mid");
    assert_eq!(ev[1]["data"]["line"], "new");
}

#[test]
fn agents_and_widgets_quiet_past_retention_are_dropped() {
    let mut d = dash();
    let (_, tok) = approved(&mut d);
    post(&mut d, &tok, "/v1/widgets", json!({"key": "s", "type": "status"}));
    let later = T0 + RETENTION_DAYS * DAY_MS + 1;
    assert_eq!(d.view(later)["agents"].as_array().unwrap().len(), 0);
    let r = d.handle(
        &Request::new("POST", "/v1/heartbeat")
            .bearer(&tok)
            .json(json!({"status": "idle"})),
        later,
    );
    assert_eq!(r.status, 410);
}

#[test]
fn idempotency_rate_limits_and_the_scanner() {
    let mut d = dash();
    let (_, tok) = approved(&mut d);
    post(&mut d, &tok, "/v1/widgets", json!({"key": "l", "type": "log"}));
    for _ in 0..3 {
        let r = d.handle(
            &Request::new("POST", "/v1/widgets/l/events")
                .bearer(&tok)
                .header("Idempotency-Key", "u1")
                .json(json!({"line": "once"})),
            T0,
        );
        assert_eq!(r.status, 200);
    }
    post(
        &mut d,
        &tok,
        "/v1/widgets/l/events",
        json!({"line": "token ghp_abcdefghijklmnopqrstuvwx here"}),
    );
    let v = d.view(T0);
    let ev = v["agents"][0]["widgets"][0]["events"].as_array().unwrap().clone();
    assert_eq!(ev.len(), 2, "the retried append landed once");
    assert_eq!(ev[1]["data"]["line"], "token [redacted] here");
    assert_eq!(v["agents"][0]["flagged"], true);
    let mut last = 0;
    for _ in 0..WRITES_PER_MIN_AGENT {
        last = post(&mut d, &tok, "/v1/heartbeat", json!({"status": "idle"})).status;
    }
    assert_eq!(last, 429);
    let r = post(&mut d, &tok, "/v1/heartbeat", json!({"status": "idle"}));
    assert!(r.body["retry_after_ms"].as_u64().unwrap() > 0);
}

#[test]
fn wrong_codes_are_rate_limited_then_force_rotation() {
    let mut d = dash();
    let good = d.issue_code(T0).unwrap();
    let try_code = |d: &mut Dash, c: &str, t: u64| {
        d.handle(
            &Request::new("POST", "/v1/enroll").json(json!({"code": c, "name": format!("a{t}")})),
            t,
        )
        .status
    };
    for i in 0..ENROLL_FAILS_PER_MIN as u64 {
        assert_eq!(try_code(&mut d, "ENR-AAAA-AAAA", T0 + i), 401);
    }
    // the 11th in the minute is refused before the code is even checked, the right one included
    assert_eq!(try_code(&mut d, "ENR-AAAA-AAAA", T0 + 20), 429);
    assert_eq!(try_code(&mut d, &good, T0 + 21), 429);
    // a minute later the good code works
    assert_eq!(try_code(&mut d, &good, T0 + 61_000), 201);
    // ~50 failures since the last rotation revoke every outstanding code
    let pending = d.issue_code(T0).unwrap();
    let mut t = T0 + 120_000;
    let mut fails = 0;
    while fails < ENROLL_FAILS_BEFORE_ROTATION - ENROLL_FAILS_PER_MIN {
        if try_code(&mut d, "ENR-AAAA-AAAA", t) == 401 {
            fails += 1;
        }
        t += 7_000;
    }
    assert_eq!(
        try_code(&mut d, &pending, t + 61_000),
        401,
        "the forced rotation killed it"
    );
    let fresh = d.issue_code(t).unwrap();
    assert_eq!(try_code(&mut d, &fresh, t + 62_000), 201);
}

/// A table state whose stored JSON is exactly `n` bytes (8192 or 8193): 16 cells of 160 three-byte chars and one of
/// 133 plus `aa` (`aaa` for one more byte). Bytes, not characters. Same builder as the Worker's test.
fn table_of(n: usize) -> Value {
    assert!(n == MAX_CONTENT_BYTES || n == MAX_CONTENT_BYTES + 1);
    let mut rows: Vec<Value> = (0..16).map(|_| json!(["\u{20ac}".repeat(160)])).collect();
    let tail = "\u{20ac}".repeat(133) + if n > MAX_CONTENT_BYTES { "aaa" } else { "aa" };
    rows.push(json!([tail]));
    let v = json!({"columns": ["c"], "rows": rows});
    assert_eq!(serde_json::to_string(&v).unwrap().len(), n);
    v
}

/// Twin of the Worker's `the content cap edge matches the Rust twin` test (QRT-307): 8192 stored bytes pass, 8193
/// are 422, for batch ops and single writes.
#[test]
fn content_cap_edge_matches_the_worker() {
    assert_eq!(MAX_CONTENT_BYTES, 8 * 1024);
    let mut d = dash();
    let (_, tok) = approved(&mut d);
    post(&mut d, &tok, "/v1/widgets", json!({"key": "t", "type": "table"}));
    let mut batch = |data: Value| {
        let r = post(
            &mut d,
            &tok,
            "/v1/batch",
            json!({"ops": [{"op": "put", "key": "t", "data": data}]}),
        );
        assert_eq!(r.status, 200, "{:?}", r.body);
        r.body["results"][0]["status"].as_u64().unwrap()
    };
    assert_eq!(batch(table_of(MAX_CONTENT_BYTES)), 200);
    assert_eq!(batch(table_of(MAX_CONTENT_BYTES + 1)), 422);
    let put = |d: &mut Dash, v: Value| {
        d.handle(&Request::new("PUT", "/v1/widgets/t").bearer(&tok).json(v), T0)
            .status
    };
    assert_eq!(put(&mut d, table_of(MAX_CONTENT_BYTES)), 200);
}

#[test]
fn audit_entries_hold_only_event_agent_id_and_time() {
    let mut d = dash();
    let (id, tok) = approved(&mut d);
    post(
        &mut d,
        &tok,
        "/v1/widgets",
        json!({"key": "w", "type": "status", "title": "secret title"}),
    );
    assert!(d.delete_widget(&id, "w", T0));
    assert!(d.delete_agent(&id, T0));
    let audit = d.view(T0)["audit"].clone();
    assert!(!audit.as_array().unwrap().is_empty());
    for e in audit.as_array().unwrap() {
        let mut keys: Vec<_> = e.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["agentId", "at", "event"], "{e}");
    }
    assert!(!audit.to_string().contains("secret title") && !audit.to_string().contains("grok"));
}

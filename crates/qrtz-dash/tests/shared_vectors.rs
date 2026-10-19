//! The vectors shared with the Cloudflare Worker (services/dash-worker/test/vectors.test.ts, ADR 0228 amendment):
//! contracts/dash/vectors.json. The scanner is reached through the public API (a status widget's text, read back
//! from the view), so both sides are held to the same rules.
use qrtz_dash::*;
use serde_json::{json, Value};

const T0: u64 = 1_800_000_000_000;

fn vectors() -> Value {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../contracts/dash/vectors.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn hmac_vectors_match() {
    for v in vectors()["hmac"].as_array().unwrap() {
        let key = hex::decode(v["key_hex"].as_str().unwrap()).unwrap();
        let mac = hmac_sha256(&key, v["msg"].as_str().unwrap().as_bytes());
        assert_eq!(hex::encode(mac), v["mac_hex"].as_str().unwrap());
    }
}

#[test]
fn https_url_vectors_match() {
    for v in vectors()["https_url"].as_array().unwrap() {
        let u = v["url"].as_str().unwrap();
        assert_eq!(is_https_url(u), v["ok"].as_bool().unwrap(), "{u}");
    }
}

#[test]
fn scrub_vectors_match() {
    let mut d = Dash::new(Pepper::from_bytes([7; 32]));
    let code = d.issue_code(T0).unwrap();
    let r = d.handle(
        &Request::new("POST", "/v1/enroll").json(json!({"code": code, "name": "grok bot"})),
        T0,
    );
    let id = r.body["agent_id"].as_str().unwrap().to_owned();
    let tok = r.body["agent_token"].as_str().unwrap().to_owned();
    assert!(d.approve(&id, T0));
    let reg = Request::new("POST", "/v1/widgets")
        .bearer(&tok)
        .json(json!({"key": "s", "type": "status"}));
    assert_eq!(d.handle(&reg, T0).status, 201);
    for (i, v) in vectors()["scrub"].as_array().unwrap().iter().enumerate() {
        let put = Request::new("PUT", "/v1/widgets/s")
            .bearer(&tok)
            .json(json!({"status": "running", "text": v["in"]}));
        // Spread the writes past the per-minute limit.
        let now = T0 + i as u64 * 2_000;
        assert_eq!(d.handle(&put, now).status, 200, "{}", v["in"]);
        let view = d.view(now);
        let w = &view["agents"][0]["widgets"][0];
        assert_eq!(w["state"]["text"], v["out"], "in: {}", v["in"]);
    }
}

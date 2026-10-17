//! Shared talk vectors (spec/vectors/talk/*.json), computed by the TS oracle (packages/core/src/talk/*.vectors.test.ts).
use qrtz_talk::arbiter::Arbiter;
use qrtz_talk::cleanup::{apply_dictionary, rule_cleanup};
use qrtz_talk::store::{parse, serialize};
use qrtz_talk::{DictionaryEntry, Engine};
use serde_json::Value;

#[test]
fn cleanup_matches_ts() {
    let v: Value = serde_json::from_str(include_str!("../../../spec/vectors/talk/cleanup.json")).unwrap();
    let dict: Vec<DictionaryEntry> = serde_json::from_value(v["dictionary"].clone()).unwrap();
    let mut n = 0;
    for r in v["rules"].as_array().unwrap() {
        let d: &[DictionaryEntry] = if r["dict"] == true { &dict } else { &[] };
        assert_eq!(
            rule_cleanup(r["raw"].as_str().unwrap(), d),
            r["expect"].as_str().unwrap(),
            "{}",
            r["raw"]
        );
        n += 1;
    }
    for r in v["dictionaryOnly"].as_array().unwrap() {
        assert_eq!(
            apply_dictionary(r["text"].as_str().unwrap(), &dict),
            r["expect"].as_str().unwrap()
        );
        n += 1;
    }
    // `plausible` (the LLM-reply check) went with talk's LLM cleanup (ADR 0071); only the TS oracle still reads it.
    assert!(n > 20);
}

fn engine(s: &str) -> Engine {
    serde_json::from_value(Value::String(s.into())).unwrap()
}

#[test]
fn engine_arbiter_matches_ts() {
    let v: Value =
        serde_json::from_str(include_str!("../../../spec/vectors/talk/engine-arbiter.json")).unwrap();
    let cases = v["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 12);
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let mut a = Arbiter::new(c["stallMs"].as_f64().unwrap_or(f64::INFINITY));
        for s in c["steps"].as_array().unwrap() {
            let at = s["at"].as_f64().unwrap();
            let t = |k: &str| s[k].as_str().unwrap_or_default().to_owned();
            match s["op"].as_str().unwrap() {
                "attach" => a.attach_cloud(engine(&t("engine"))),
                "cloud" => a.cloud_update(&t("committed"), &t("partial")),
                "cloudError" => {
                    a.cloud_error(&t("error"));
                }
                "localPartial" => {
                    a.local_partial(&t("text"), at);
                }
                "localFinal" => {
                    a.local_final(&t("text"), at);
                }
                _ => {
                    a.check(at);
                }
            }
            let e = &s["expect"];
            let (vc, vp) = a.view();
            assert_eq!(a.using_cloud(), e["usingCloud"].as_bool().unwrap(), "{name} {s}");
            assert_eq!(a.engine(), engine(e["engine"].as_str().unwrap()), "{name} {s}");
            assert_eq!(
                [vc, vp],
                [e["view"][0].as_str().unwrap(), e["view"][1].as_str().unwrap()],
                "{name} {s}"
            );
        }
        let f = &c["finish"];
        let cloud = match &f["cloud"] {
            Value::Null => None,
            x if x.get("ok").is_some() => Some(Ok(x["ok"].as_str().unwrap().to_owned())),
            x => {
                // TS: the flush times out after the provider already reported the error.
                a.cloud_error(x["err"].as_str().unwrap());
                Some(Err(x["err"].as_str().unwrap().to_owned()))
            }
        };
        let cloud = if a.using_cloud() || cloud.as_ref().is_some_and(|c| c.is_err()) {
            cloud
        } else {
            None
        };
        let r = a.finish(f["local"].as_str().unwrap(), cloud);
        let e = &c["expect"];
        assert_eq!(r.text, e["text"].as_str().unwrap(), "{name}");
        assert_eq!(r.engine, engine(e["engine"].as_str().unwrap()), "{name}");
        assert_eq!(r.failover, e["failover"].as_bool().unwrap(), "{name}");
        assert_eq!(r.reason.as_deref(), e["reason"].as_str(), "{name}");
    }
}

#[test]
fn store_matches_ts() {
    let v: Value = serde_json::from_str(include_str!("../../../spec/vectors/talk/store.json")).unwrap();
    let (st, dropped) = parse(v["file"].to_string().as_bytes()).unwrap();
    let e = &v["expect"];
    assert_eq!(
        st.history.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
        e["history"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        st.dictionary.iter().map(|d| d.term.as_str()).collect::<Vec<_>>(),
        e["dictionary"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(dropped as u64, e["dropped"].as_u64().unwrap());
    assert_eq!(
        String::from_utf8(serialize(&st)).unwrap(),
        e["serialized"].as_str().unwrap(),
        "byte-identical to JSON.stringify"
    );
}

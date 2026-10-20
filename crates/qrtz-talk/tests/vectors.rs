#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Shared talk vectors (spec/vectors/talk/*.json), computed by the TS oracle (packages/core/src/talk/*.vectors.test.ts).
use qrtz_talk::cleanup::{apply_dictionary, rule_cleanup};
use qrtz_talk::store::{parse, serialize};
use qrtz_talk::DictionaryEntry;
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
    assert!(n > 20);
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

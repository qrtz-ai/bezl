//! Runs the shared tier-1 vectors (spec/vectors/lifecycle/*.json) against the Rust port. The same files are run
//! by vitest (packages/core/src/note/lifecycle.vectors.test.ts), and TS regenerates them, so passing here means
//! byte-for-byte agreement with the TS lifecycle. Comparison is on `serde_json::Value` (key order irrelevant).

use std::path::PathBuf;

use qrtz_lifecycle::model::{LifecycleEvent, PipelineContext, RecordingState, StepId, StopReason};
use qrtz_lifecycle::{
    initial_state, new_pipeline, next_runnable_step, pipeline_settled, pipeline_step, pipeline_usable,
    policies, reduce, retry_delay, Machine, RetryPolicy, StepPatch,
};
use serde_json::{json, Value};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors/lifecycle")
}

fn load() -> Vec<(String, Value)> {
    let mut out: Vec<_> = std::fs::read_dir(dir())
        .expect("spec/vectors/lifecycle")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            (name, v)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn from<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{e}: {v}"))
}

fn to(v: &impl serde::Serialize) -> Value {
    serde_json::to_value(v).unwrap()
}

fn outcome(s: &RecordingState, e: &LifecycleEvent) -> (Value, Option<RecordingState>) {
    match reduce(s, e) {
        Some(n) => (to(&n), Some(n)),
        None => (json!("rejected"), None),
    }
}

fn run_session(name: &str, v: &Value) -> usize {
    let initial = if v["initial"].is_null() {
        initial_state()
    } else {
        from(&v["initial"])
    };
    let mut m = Machine::new(initial);
    let mut n = 0;
    for (i, st) in v["steps"].as_array().unwrap().iter().enumerate() {
        let e: LifecycleEvent = from(&st["event"]);
        // Every event must round-trip through serde exactly as TS wrote it.
        assert_eq!(to(&e), st["event"], "{name} step {i}: event round trip");
        let (got, _) = outcome(m.state(), &e);
        assert_eq!(got, st["expect"], "{name} step {i} ({})", st["event"]["type"]);
        m.send(&e);
        n += 1;
    }
    n
}

fn run_matrix(name: &str, v: &Value) -> usize {
    let events: Vec<LifecycleEvent> = from(&v["events"]);
    let mut n = 0;
    for (state_name, raw) in v["states"].as_object().unwrap() {
        let s: RecordingState = from(raw);
        let expect = v["expect"][state_name].as_array().unwrap();
        assert_eq!(expect.len(), events.len());
        for (e, want) in events.iter().zip(expect) {
            let (got, _) = outcome(&s, e);
            assert_eq!(&got, want, "{name}: {state_name} + {}", to(e)["type"]);
            n += 1;
        }
    }
    n
}

fn run_pipeline(name: &str, v: &Value) -> usize {
    let i = &v["initial"];
    let ctx: PipelineContext = from(&i["ctx"]);
    let reason: StopReason = from(&i["stopReason"]);
    let mut p = new_pipeline(
        i["meetingId"].as_str().unwrap(),
        reason,
        ctx,
        i["now"].as_i64().unwrap(),
    );
    let mut n = 0;
    for (k, st) in v["steps"].as_array().unwrap().iter().enumerate() {
        let id: StepId = from(&st["step"]);
        let patch: StepPatch = from(&st["patch"]);
        assert_eq!(to(&patch), st["patch"], "{name} step {k}: patch round trip");
        p = pipeline_step(&p, id, patch);
        let want = &st["expect"];
        assert_eq!(to(&p), want["state"], "{name} step {k} state");
        assert_eq!(
            to(&next_runnable_step(&p)),
            want["nextRunnable"],
            "{name} step {k} nextRunnable"
        );
        assert_eq!(
            json!(pipeline_settled(&p)),
            want["settled"],
            "{name} step {k} settled"
        );
        assert_eq!(
            json!(pipeline_usable(&p)),
            want["usable"],
            "{name} step {k} usable"
        );
        // Persisted JSON written by either side reads back identically.
        assert_eq!(
            to(&from::<qrtz_lifecycle::model::PipelineState>(&want["state"])),
            want["state"]
        );
        n += 1;
    }
    n
}

fn run_retry_delay(name: &str, v: &Value) -> usize {
    let cases = v["cases"].as_array().unwrap();
    for c in cases {
        let p: RetryPolicy = from(&c["policy"]);
        let got = retry_delay(
            &p,
            c["attempt"].as_u64().unwrap() as u32,
            c["random"].as_f64().unwrap(),
            c["scale"].as_f64().unwrap(),
        );
        assert_eq!(json!(got), c["expect"], "{name}: {c}");
    }
    cases.len()
}

fn run_policies(v: &Value) -> usize {
    let e = &v["expect"];
    assert_eq!(
        e["helperReconnectWindowMs"],
        json!(policies::HELPER_RECONNECT_WINDOW_MS)
    );
    assert_eq!(e["startAckMs"], json!(policies::START_ACK_MS));
    assert_eq!(e["stopAckMs"], json!(policies::STOP_ACK_MS));
    assert_eq!(e["abortAckMs"], json!(policies::ABORT_ACK_MS));
    assert_eq!(e["appendLive"], to(&policies::APPEND_LIVE));
    assert_eq!(e["finalize"], to(&policies::FINALIZE));
    assert_eq!(e["summary"], to(&policies::SUMMARY));
    assert_eq!(e["index"], to(&policies::INDEX));
    assert_eq!(
        e.as_object().unwrap().len(),
        8,
        "a policy was added on one side only"
    );
    8
}

#[test]
fn shared_lifecycle_vectors() {
    let vectors = load();
    assert!(vectors.len() > 5, "no vectors found in {:?}", dir());
    let mut checks = 0;
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    for (name, v) in &vectors {
        let kind = v["kind"].as_str().unwrap();
        assert!(
            !v.to_string().contains("\"expect\":null"),
            "{name}: run the TS update first"
        );
        checks += match kind {
            "session" => run_session(name, v),
            "session-matrix" => run_matrix(name, v),
            "pipeline" => run_pipeline(name, v),
            "retry-delay" => run_retry_delay(name, v),
            "policies" => run_policies(v),
            other => panic!("{name}: unknown vector kind {other}"),
        };
        *kinds.entry(kind.to_string()).or_default() += 1;
    }
    eprintln!("{} vector files, {checks} checks: {kinds:?}", vectors.len());
}

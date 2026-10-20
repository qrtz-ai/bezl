#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Electron <-> Rust database cross-open. Mirror of apps/desktop/src/worker/db-parity.test.ts: the ops in
//! spec/fixtures/db/seed.json, applied through `NoteRepo`/`LifecycleRepo`, must dump to expected.json (written by
//! the TS oracle); electron.sqlite (made by node:sqlite) must open here and dump the same; rust.sqlite (made here)
//! is what the TS side opens.
//!
//! `QRTZ_FIXTURES_UPDATE=1 cargo test -p qrtz-storage --test parity` rewrites rust.sqlite.

use std::path::{Path, PathBuf};

use qrtz_storage::model::{
    Meeting, MeetingNote, MeetingStatus, MeetingSummary, PipelineState, RecordingState, Segment,
    SegmentSource,
};
use qrtz_storage::{to_fts_query, Db, ListQuery, Result, StorageError, VectorRow};
use serde::Serialize;
use serde_json::{json, Map, Value};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/fixtures/db")
}

fn read_json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir().join(name)).unwrap()).unwrap()
}

fn de<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("seed: {e} in {v}"))
}

fn s<'v>(v: &'v Value, k: &str) -> &'v str {
    v[k].as_str().unwrap_or_else(|| panic!("seed: missing {k}"))
}

fn to_val<T: Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn rowid(db: &Db, meeting_id: &str, source: &str, id: &str) -> i64 {
    db.connection()
        .query_row(
            "SELECT rowid FROM segments WHERE meeting_id=? AND source=? AND id=?",
            [meeting_id, source, id],
            |r| r.get(0),
        )
        .unwrap()
}

fn apply(db: &Db, ops: &[Value]) -> Vec<Value> {
    let (repo, life) = (db.notes(), db.lifecycle());
    let mut out = Vec::new();
    for o in ops {
        match s(o, "op") {
            "upsertMeeting" => repo.upsert_meeting(&de::<Meeting>(&o["meeting"])).unwrap(),
            "setTemplate" => repo.set_template(s(o, "id"), s(o, "template")).unwrap(),
            "setStatus" => repo
                .set_status(s(o, "id"), de::<MeetingStatus>(&o["status"]))
                .unwrap(),
            "replaceSegments" => repo
                .replace_segments(
                    s(o, "meetingId"),
                    de::<SegmentSource>(&o["source"]),
                    &de::<Vec<Segment>>(&o["segments"]),
                )
                .unwrap(),
            "appendSegment" => repo
                .append_segment(s(o, "meetingId"), &de::<Segment>(&o["segment"]))
                .unwrap(),
            "renameSpeaker" => out.push(json!(repo
                .rename_speaker(s(o, "meetingId"), s(o, "from"), s(o, "to"))
                .unwrap())),
            "addNote" => repo
                .add_note(s(o, "meetingId"), &de::<MeetingNote>(&o["note"]))
                .unwrap(),
            "updateNote" => out.push(json!(repo.update_note(s(o, "id"), s(o, "text")).unwrap())),
            "deleteNote" => out.push(json!(repo.delete_note(s(o, "id")).unwrap())),
            "saveSummary" => repo
                .save_summary(
                    s(o, "meetingId"),
                    &de::<MeetingSummary>(&o["summary"]),
                    o["now"].as_i64().unwrap(),
                )
                .unwrap(),
            "deleteMeeting" => repo.delete_meeting(s(o, "id")).unwrap(),
            "saveSession" => life
                .save_session(&de::<RecordingState>(&o["state"]), o["now"].as_i64().unwrap())
                .unwrap(),
            "savePipeline" => out.push(json!(life
                .save_pipeline(&de::<PipelineState>(&o["state"]), o["settled"].as_bool().unwrap())
                .unwrap())),
            "saveVectors" => {
                let rows: Vec<VectorRow> = o["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| VectorRow {
                        rowid: rowid(db, s(r, "meetingId"), s(r, "source"), s(r, "segmentId")),
                        meeting_id: s(r, "meetingId").into(),
                        vec: unhex(s(r, "vecHex")),
                    })
                    .collect();
                repo.save_vectors(s(o, "model"), &rows).unwrap()
            }
            other => panic!("seed: unknown op {other}"),
        }
    }
    out
}

fn attempt<T: Serialize>(r: Result<T>) -> Value {
    match r {
        Ok(v) => to_val(v),
        Err(e) => json!({ "error": e.code() }),
    }
}

fn dump(db: &Db, seed: &Value, op_results: &[Value]) -> Value {
    let (repo, life) = (db.notes(), db.lifecycle());
    let q = &seed["queries"];
    let arr = |k: &str| q[k].as_array().unwrap().clone();
    let opt = |v: &Value, k: &str| v[k].as_str().map(str::to_owned);
    let mut schema = Vec::new();
    {
        let mut st = db
            .connection()
            .prepare("SELECT type, name, tbl_name AS tbl, sql FROM sqlite_master ORDER BY type, name")
            .unwrap();
        let mut rows = st.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            let sql: Option<String> = r.get(3).unwrap();
            schema.push(json!({
                "type": r.get::<_, String>(0).unwrap(),
                "name": r.get::<_, String>(1).unwrap(),
                "tbl": r.get::<_, String>(2).unwrap(),
                "sql": sql,
            }));
        }
    }
    let by_id = |k: &str, f: &dyn Fn(&str) -> Value| -> Value {
        Value::Object(
            arr(k)
                .iter()
                .map(|id| (id.as_str().unwrap().to_owned(), f(id.as_str().unwrap())))
                .collect::<Map<_, _>>(),
        )
    };
    json!({
        "opResults": op_results,
        "userVersion": db.schema_version().unwrap(),
        "health": to_val(db.health().unwrap()),
        "schema": schema,
        "list": arr("list").iter().map(|p| to_val(repo.list(&de::<ListQuery>(p)).unwrap())).collect::<Vec<_>>(),
        "details": by_id("details", &|id| attempt(repo.detail(id))),
        "searchSegments": arr("searchSegments").iter()
            .map(|t| to_val(repo.search_segments(t.as_str().unwrap(), 60).unwrap())).collect::<Vec<_>>(),
        "missingVectors": arr("missingVectors").iter().map(|p| to_val(repo.missing_vectors(
            s(p, "model"), p["limit"].as_u64().unwrap() as u32, opt(p, "meetingId").as_deref()).unwrap()))
            .collect::<Vec<_>>(),
        "vectorSources": arr("vectorSources").iter().map(|p| {
            let rows = repo.vector_sources(s(p, "model"), opt(p, "meetingId").as_deref()).unwrap();
            Value::Array(rows.iter().map(|v| {
                let mut o = to_val(v);
                o["vec"] = json!(hex(&v.vec));
                o
            }).collect())
        }).collect::<Vec<_>>(),
        "session": to_val(life.load_session().unwrap()),
        "pipelines": by_id("pipelines", &|id| to_val(life.get_pipeline(id).unwrap())),
        "unsettled": to_val(life.list_unsettled().unwrap()),
        "orphans": to_val(life.orphans().unwrap()),
        "notes": by_id("notes", &|id| to_val(repo.get_notes(id).unwrap())),
        "ftsQueries": arr("ftsQueries").iter().map(|t| to_val(to_fts_query(t.as_str().unwrap()))).collect::<Vec<_>>(),
    })
}

/// Points at the first differing path, so a failure is readable without diffing 30 KB by eye.
fn first_diff(a: &Value, b: &Value, path: String) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys()) {
                let (l, r) = (x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null));
                if x.contains_key(k) != y.contains_key(k) {
                    return Some(format!("{path}.{k}: key only on one side ({l} vs {r})"));
                }
                if let Some(d) = first_diff(l, r, format!("{path}.{k}")) {
                    return Some(d);
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(i, (l, r))| first_diff(l, r, format!("{path}[{i}]"))),
        // One JS number type: 1.0 from serde and 1 from JSON.stringify are the same value on the wire.
        (Value::Number(x), Value::Number(y)) if x.as_f64() == y.as_f64() => None,
        _ if a == b => None,
        _ => Some(format!("{path}: got {a} expected {b}")),
    }
}

fn assert_same(got: &Value, expected: &Value, what: &str) {
    if let Some(d) = first_diff(got, expected, String::new()) {
        panic!("{what} differs from expected.json at {d}");
    }
}

struct Fixture {
    seed: Value,
    expected: Value,
    results: Vec<Value>,
    tmp: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let seed = read_json("seed.json");
    let probe = Db::open_in_memory().unwrap();
    let results = apply(&probe, seed["ops"].as_array().unwrap());
    Fixture {
        expected: read_json("expected.json"),
        seed,
        results,
        tmp: tempfile::tempdir().unwrap(),
    }
}

impl Fixture {
    /// Scratch copy (opening switches a DB to WAL; the committed files stay byte-stable).
    fn open_copy(&self, name: &str) -> Db {
        let p = self.tmp.path().join(name);
        std::fs::copy(dir().join(name), &p).unwrap();
        Db::open(&p).unwrap()
    }
}

#[test]
fn fresh_rust_db_matches_the_ts_oracle() {
    let f = fixture();
    let db = Db::open_in_memory().unwrap();
    let results = apply(&db, f.seed["ops"].as_array().unwrap());
    if std::env::var("QRTZ_FIXTURES_UPDATE").as_deref() == Ok("1") {
        let out = dir().join("rust.sqlite");
        let _ = std::fs::remove_file(&out);
        let file = Db::open(&out).unwrap();
        apply(&file, f.seed["ops"].as_array().unwrap());
        file.connection()
            .query_row("PRAGMA journal_mode = DELETE", [], |_| Ok(()))
            .unwrap();
    }
    assert_same(&dump(&db, &f.seed, &results), &f.expected, "fresh Rust DB");
}

#[test]
fn opens_the_electron_created_db() {
    let f = fixture();
    let db = f.open_copy("electron.sqlite");
    assert_eq!(
        db.migration().unwrap().applied,
        ["6_calendar"],
        "the Electron DB (schema 5, the last it shipped) gains only the calendar cache (ADR 0217)"
    );
    assert_same(
        &dump(&db, &f.seed, &f.results),
        &f.expected,
        "electron.sqlite read by Rust",
    );
}

#[test]
fn opens_the_rust_created_db() {
    let f = fixture();
    let db = f.open_copy("rust.sqlite");
    assert_same(
        &dump(&db, &f.seed, &f.results),
        &f.expected,
        "rust.sqlite read by Rust",
    );
}

#[test]
fn keeps_writing_to_an_electron_created_db() {
    let f = fixture();
    let db = f.open_copy("electron.sqlite");
    let repo = db.notes();
    repo.add_note(
        "m-b",
        &MeetingNote {
            id: "n9".into(),
            at_ms: 9,
            text: "added by Rust".into(),
            created_at: 9,
        },
    )
    .unwrap();
    let seg: Segment =
        de(&json!({"id":"c0","startMs":0,"endMs":10,"text":"Rust friday append","source":"live"}));
    repo.append_segment("m-c", &seg).unwrap();
    let c0 = rowid(&db, "m-c", "live", "c0");
    repo.save_vectors(
        "minilm-q8",
        &[VectorRow {
            rowid: c0,
            meeting_id: "m-c".into(),
            vec: vec![7],
        }],
    )
    .unwrap();
    let ids = |q: &str| -> Vec<String> {
        repo.list(&ListQuery {
            query: Some(q.into()),
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .map(|r| r.meeting.id)
        .collect()
    };
    assert_eq!(ids("rust friday"), ["m-c"]);
    assert_eq!(
        repo.get_notes("m-b")
            .unwrap()
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>(),
        ["n4", "n9"]
    );
    assert_eq!(
        hex(&repo.vector_sources("minilm-q8", Some("m-c")).unwrap()[0].vec),
        "07"
    );
    repo.delete_meeting("m-a").unwrap();
    assert!(ids("rollback").is_empty());
    assert!(repo.vector_sources("minilm-q8", Some("m-a")).unwrap().is_empty());
    assert!(db.lifecycle().get_pipeline("m-a").unwrap().is_none());
    assert!(matches!(repo.need("m-a"), Err(StorageError::NotFound { .. })));
    let ok: String = db
        .connection()
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
}

#[test]
fn rust_schema_is_byte_identical_to_electron() {
    let f = fixture();
    let db = Db::open_in_memory().unwrap();
    assert_same(
        &dump(&db, &f.seed, &[])["schema"],
        &f.expected["schema"],
        "fresh Rust schema",
    );
}

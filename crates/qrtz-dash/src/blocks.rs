//! The eight typed blocks (spec "Block types"). Payloads are parsed with serde (`deny_unknown_fields`), then the
//! limits the types can't say are checked. The zod twin is packages/core/src/dash (both read the same samples).
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockType {
    Status,
    Stat,
    Progress,
    List,
    Table,
    Log,
    Chart,
    Markdown,
}

impl BlockType {
    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(Value::String(s.to_owned())).ok()
    }
    /// log and chart take appends (`POST .../events`); the rest take PUT.
    pub fn is_append(self) -> bool {
        matches!(self, BlockType::Log | BlockType::Chart)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct BlockError(pub String);

fn err(m: impl Into<String>) -> BlockError {
    BlockError(m.into())
}

pub const TEXT_MAX: usize = 500;
pub const LIST_MAX: usize = 50;
pub const TABLE_ROWS_MAX: usize = 100;
pub const TABLE_COLS_MAX: usize = 20;
pub const MARKDOWN_MAX: usize = 2000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    status: StatusKind,
    text: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum StatusKind {
    Running,
    Blocked,
    WaitingOnUser,
    Done,
    Error,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stat {
    value: f64,
    label: Option<String>,
    unit: Option<String>,
    delta: Option<f64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    done: u64,
    total: u64,
    step: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    items: Vec<ListItem>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListItem {
    text: String,
    url: Option<String>,
    state: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    columns: Vec<String>,
    rows: Vec<Vec<Cell>>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Cell {
    S(String),
    N(f64),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Log {
    line: String,
    level: Option<Level>,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Level {
    Info,
    Warn,
    Error,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Chart {
    value: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Markdown {
    text: String,
}

fn parse<T: for<'de> Deserialize<'de>>(v: &Value) -> Result<T, BlockError> {
    serde_json::from_value(v.clone()).map_err(|e| err(format!("invalid payload: {e}")))
}

fn text(s: &str, max: usize, what: &str) -> Result<(), BlockError> {
    if s.chars().count() > max {
        return Err(err(format!("{what} is longer than {max} characters")));
    }
    Ok(())
}

fn opt(s: &Option<String>, what: &str) -> Result<(), BlockError> {
    s.as_deref().map_or(Ok(()), |s| text(s, TEXT_MAX, what))
}

fn finite(n: f64, what: &str) -> Result<(), BlockError> {
    if n.is_finite() {
        Ok(())
    } else {
        Err(err(format!("{what} must be a finite number")))
    }
}

/// `https://<host>[/...]` with a real-looking host and nothing else (no javascript:, data:, http:, userinfo).
pub fn is_https_url(u: &str) -> bool {
    let Some(rest) = u.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty()
        && !host.contains('@')
        && host.contains('.')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
        && !u.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Safe subset: no HTML tags, no images, no autolinks (`<https://...>`).
fn markdown_ok(s: &str) -> Result<(), BlockError> {
    text(s, MARKDOWN_MAX, "text")?;
    let b = s.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'<'
            && b.get(i + 1)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'/' || *c == b'!')
        {
            return Err(err("markdown can't contain html or autolinks"));
        }
        if b[i] == b'!' && b.get(i + 1) == Some(&b'[') {
            return Err(err("markdown can't contain images"));
        }
    }
    Ok(())
}

/// Validate a PUT payload for `ty`. Returns the payload unchanged when it is valid.
pub fn validate_put(ty: BlockType, v: &Value) -> Result<(), BlockError> {
    match ty {
        BlockType::Status => {
            let s: Status = parse(v)?;
            let _ = s.status;
            opt(&s.text, "text")
        }
        BlockType::Stat => {
            let s: Stat = parse(v)?;
            finite(s.value, "value")?;
            s.delta.map_or(Ok(()), |d| finite(d, "delta"))?;
            opt(&s.label, "label")?;
            opt(&s.unit, "unit")
        }
        BlockType::Progress => {
            let p: Progress = parse(v)?;
            if p.done > p.total {
                return Err(err("done is more than total"));
            }
            opt(&p.step, "step")
        }
        BlockType::List => {
            let l: List = parse(v)?;
            if l.items.len() > LIST_MAX {
                return Err(err(format!("a list has at most {LIST_MAX} items")));
            }
            for it in &l.items {
                text(&it.text, TEXT_MAX, "text")?;
                opt(&it.state, "state")?;
                if let Some(u) = &it.url {
                    if !is_https_url(u) {
                        return Err(err("list urls must be https://"));
                    }
                }
            }
            Ok(())
        }
        BlockType::Table => {
            let t: Table = parse(v)?;
            if t.rows.len() > TABLE_ROWS_MAX || t.columns.len() > TABLE_COLS_MAX {
                return Err(err(format!(
                    "a table has at most {TABLE_ROWS_MAX} rows and {TABLE_COLS_MAX} columns"
                )));
            }
            for c in &t.columns {
                text(c, TEXT_MAX, "column")?;
            }
            for r in &t.rows {
                if r.len() != t.columns.len() {
                    return Err(err("every row has one cell per column"));
                }
                for c in r {
                    match c {
                        Cell::S(s) => text(s, TEXT_MAX, "cell")?,
                        Cell::N(n) => finite(*n, "cell")?,
                    }
                }
            }
            Ok(())
        }
        BlockType::Markdown => markdown_ok(&parse::<Markdown>(v)?.text),
        BlockType::Log | BlockType::Chart => Err(err("this widget takes events, not put")),
    }
}

/// Validate an appended event for `ty` (log or chart).
pub fn validate_append(ty: BlockType, v: &Value) -> Result<(), BlockError> {
    match ty {
        BlockType::Log => {
            let l: Log = parse(v)?;
            let _ = l
                .level
                .map(|l| matches!(l, Level::Info | Level::Warn | Level::Error));
            text(&l.line, TEXT_MAX, "line")
        }
        BlockType::Chart => finite(parse::<Chart>(v)?.value, "value"),
        _ => Err(err("this widget takes put, not events")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_block_accepts_its_spec_example_and_refuses_extras() {
        use BlockType::*;
        let ok = [
            (Status, json!({"status": "blocked", "text": "Waiting on User ID"})),
            (
                Stat,
                json!({"value": 3, "label": "open", "unit": "pr", "delta": -1}),
            ),
            (Progress, json!({"done": 2, "total": 5, "step": "upload"})),
            (
                List,
                json!({"items": [{"text": "a", "url": "https://example.com/x", "state": "done"}]}),
            ),
            (Table, json!({"columns": ["a", "b"], "rows": [["x", 1]]})),
            (Markdown, json!({"text": "**hi** [docs](https://qrtz.ai)"})),
        ];
        for (t, v) in ok {
            assert_eq!(validate_put(t, &v), Ok(()), "{t:?}");
            let mut extra = v.clone();
            extra["action"] = json!("click me");
            assert!(validate_put(t, &extra).is_err(), "{t:?} extra field");
        }
        assert!(validate_append(Log, &json!({"line": "x", "level": "warn"})).is_ok());
        assert!(validate_append(Chart, &json!({"value": 1.5})).is_ok());
        assert!(validate_put(Log, &json!({"line": "x"})).is_err());
        assert!(validate_append(Status, &json!({"status": "done"})).is_err());
    }

    #[test]
    fn limits_and_unsafe_content_are_refused() {
        use BlockType::*;
        assert!(validate_put(Status, &json!({"status": "needs_you"})).is_err());
        assert!(validate_put(Progress, &json!({"done": 6, "total": 5})).is_err());
        let items: Vec<_> = (0..51).map(|_| json!({"text": "x"})).collect();
        assert!(validate_put(List, &json!({ "items": items })).is_err());
        for u in [
            "http://a.com",
            "javascript:alert(1)",
            "data:text/html,x",
            "https://user@a.com",
            "https://localhost",
            "https://a .com",
        ] {
            assert!(
                validate_put(List, &json!({"items": [{"text": "x", "url": u}]})).is_err(),
                "{u}"
            );
        }
        for m in [
            "<b>x</b>",
            "![i](https://a.com/i.png)",
            "<https://a.com>",
            "<!-- x -->",
        ] {
            assert!(validate_put(Markdown, &json!({ "text": m })).is_err(), "{m}");
        }
        assert!(validate_put(Markdown, &json!({"text": "a < b"})).is_ok());
        assert!(validate_put(Markdown, &json!({"text": "x".repeat(2001)})).is_err());
        assert!(validate_put(Table, &json!({"columns": ["a"], "rows": [["x", "y"]]})).is_err());
        assert!(BlockType::parse("action").is_none());
    }
}

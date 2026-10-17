//! Lenient row readers. node:sqlite stores whatever JS number it was given: an INTEGER-affinity column can hold a
//! REAL if TS ever wrote a fractional value, so integer reads accept REAL and round (docs/rust-core-api.md §3.2).

use rusqlite::types::ValueRef;
use rusqlite::Row;

use crate::{Result, StorageError};

fn bad(col: &str, what: &str) -> StorageError {
    StorageError::Corrupt(format!("column {col}: expected {what}"))
}

pub fn opt_i64(r: &Row, col: &str) -> Result<Option<i64>> {
    Ok(match r.get_ref(col)? {
        ValueRef::Null => None,
        ValueRef::Integer(i) => Some(i),
        ValueRef::Real(f) => Some(f.round() as i64),
        ValueRef::Text(t) => Some(
            std::str::from_utf8(t)
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok())
                .ok_or_else(|| bad(col, "number"))?
                .round() as i64,
        ),
        ValueRef::Blob(_) => return Err(bad(col, "number")),
    })
}

pub fn i64_(r: &Row, col: &str) -> Result<i64> {
    opt_i64(r, col)?.ok_or_else(|| bad(col, "non-null number"))
}

pub fn opt_f64(r: &Row, col: &str) -> Result<Option<f64>> {
    Ok(match r.get_ref(col)? {
        ValueRef::Null => None,
        ValueRef::Integer(i) => Some(i as f64),
        ValueRef::Real(f) => Some(f),
        _ => return Err(bad(col, "number")),
    })
}

/// `String(x)` for a non-null column (numbers are stringified like JS would for integers).
pub fn string(r: &Row, col: &str) -> Result<String> {
    opt_string(r, col)?.ok_or_else(|| bad(col, "non-null text"))
}

pub fn opt_string(r: &Row, col: &str) -> Result<Option<String>> {
    Ok(match r.get_ref(col)? {
        ValueRef::Null => None,
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Integer(i) => Some(i.to_string()),
        ValueRef::Real(f) => Some(f.to_string()),
        ValueRef::Blob(b) => Some(String::from_utf8_lossy(b).into_owned()),
    })
}

pub fn blob(r: &Row, col: &str) -> Result<Vec<u8>> {
    Ok(match r.get_ref(col)? {
        ValueRef::Blob(b) => b.to_vec(),
        ValueRef::Text(t) => t.to_vec(),
        _ => return Err(bad(col, "blob")),
    })
}

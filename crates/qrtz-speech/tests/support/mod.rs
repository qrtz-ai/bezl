//! Test support: re-exports the crate's test kit; `mock_bundle` uses this crate's `qrtz-mock-speech` binary.
#![allow(dead_code, unused_imports)]
pub use qrtz_speech::testkit::*;

pub fn mock_bundle(
    sk: &ed25519_dalek::SigningKey,
    key_id: &str,
    version: &str,
    cfg: serde_json::Value,
    expected: &str,
) -> Bundle {
    let entry = std::fs::read(env!("CARGO_BIN_EXE_qrtz-mock-speech")).unwrap();
    mock_bundle_with(sk, key_id, version, cfg, expected, entry)
}

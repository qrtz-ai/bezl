//! The dev-only loopback origin (docs/dash-local.md). FOR BOZNIAK REVIEW.
//!
//! This is the only file in qrtz-dash-net that may name a non-https URL, and lib.rs compiles it only under
//! `#[cfg(all(debug_assertions, feature = "dev-local"))]`. qrtz-lifecycle's offline_graph test allows this exact
//! path under that exact cfg (docs/modules/confinement-map.json `devOnly`) and nothing else; release builds never
//! contain it.

/// The only dev origin shape accepted, followed by an explicit port.
pub const DEV_ORIGIN_PREFIX: &str = "http://127.0.0.1:";

/// `http://127.0.0.1:<port>` exactly: explicit port 1-65535, no path, userinfo, query, fragment or other host
/// (not `localhost`, not `[::1]`).
pub fn dev_origin_ok(o: &str) -> bool {
    let Some(port) = o.strip_prefix(DEV_ORIGIN_PREFIX) else {
        return false;
    };
    !port.is_empty()
        && port.len() <= 5
        && port.bytes().all(|b| b.is_ascii_digit())
        && !port.starts_with('0')
        && port.parse::<u32>().is_ok_and(|p| (1..=65535).contains(&p))
}

/// The override, read once. `Err` if it is set but not a valid loopback origin (the helper then exits).
pub(crate) fn read() -> Result<Option<&'static str>, ()> {
    static O: std::sync::OnceLock<Result<Option<String>, ()>> = std::sync::OnceLock::new();
    O.get_or_init(|| match std::env::var(crate::DEV_ORIGIN_VAR) {
        Err(std::env::VarError::NotPresent) => Ok(None),
        Ok(v) if dev_origin_ok(&v) => Ok(Some(v)),
        _ => Err(()),
    })
    .as_ref()
    .map(|o| o.as_deref())
    .map_err(|_| ())
}

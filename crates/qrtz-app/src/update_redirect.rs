//! QRT-313 (Bozniak's design): where qrtz's own update may be downloaded from. The feed and the archives are GitHub
//! release assets of `qrtz-ai/qrtz`; GitHub answers `https://github.com/qrtz-ai/qrtz/releases/...` with a redirect
//! to its asset storage. The updater follows those redirects by hand and judges every hop with [`allow`]:
//!
//! - every hop must be `https` (no plain http anywhere in the chain), with no user info and no explicit port;
//! - the URL that finally answers must be on exactly `objects.githubusercontent.com` or
//!   `release-assets.githubusercontent.com` (exact host match: no suffix, prefix or substring matching, so neither
//!   `objects.githubusercontent.com.evil.com` nor `evilobjects.githubusercontent.com` passes);
//! - a hop before the last may also be exactly `github.com` (where the chain starts, and where
//!   `releases/latest/download/…` redirects to the tagged release), never anything else.
//!
//! Anything else stops the download with [`RedirectBlocked`] (error code [`CODE`], `update_redirect_blocked`),
//! naming the refused host and scheme for the settings row. This only decides where bytes may come from: every
//! signature, sha256, size, expiry, version and codesign check in `self_update.rs` still applies to them.

/// The error code the settings row keys its message on.
pub const CODE: &str = "update_redirect_blocked";
/// Where the updater's requests start (and the only host allowed before the last hop besides [`FINAL_HOSTS`]).
pub const START_HOST: &str = "github.com";
/// The only hosts a download may finally come from.
pub const FINAL_HOSTS: [&str; 2] = [
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// A hop (or the final answer) the policy refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectBlocked {
    /// The refused URL's host (`""` when it has none).
    pub host: String,
    /// The refused URL's scheme (`""` when it doesn't parse).
    pub scheme: String,
}

impl RedirectBlocked {
    /// The refused `url`'s host and scheme.
    pub fn of(url: &str) -> Self {
        match url::Url::parse(url) {
            Ok(u) => Self {
                host: u.host_str().unwrap_or_default().to_owned(),
                scheme: u.scheme().to_owned(),
            },
            Err(_) => Self {
                host: String::new(),
                scheme: String::new(),
            },
        }
    }

    /// The `Failed` message the updater keeps: `update_redirect_blocked: <scheme>://<host>`.
    pub fn message(&self) -> String {
        format!("{CODE}: {}://{}", self.scheme, self.host)
    }

    /// Back from [`RedirectBlocked::message`] (the updater's errors are strings); `None` for any other message.
    pub fn from_message(m: &str) -> Option<Self> {
        let rest = m.strip_prefix(CODE)?.strip_prefix(": ")?;
        let (scheme, host) = rest.split_once("://")?;
        if scheme.contains(char::is_whitespace) || host.contains(char::is_whitespace) {
            return None;
        }
        Some(Self {
            host: host.to_owned(),
            scheme: scheme.to_owned(),
        })
    }
}

impl std::fmt::Display for RedirectBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// May the updater request `url` (`last == false`: a hop, asked before it is requested), or read the body of the
/// URL that finally answered (`last == true`)?
pub fn allow(url: &str, last: bool) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() || u.port().is_some() {
        return false;
    }
    let Some(url::Host::Domain(host)) = u.host() else {
        return false;
    };
    FINAL_HOSTS.contains(&host) || (!last && host == START_HOST)
}

/// The whole chain `urls` (the first request, then each redirect's target, the last one the URL that answered):
/// `Ok` when every hop passes [`allow`], else the first refused hop.
pub fn check_chain(urls: &[&str]) -> Result<(), RedirectBlocked> {
    let Some(n) = urls.len().checked_sub(1) else {
        return Err(RedirectBlocked::of(""));
    };
    for (i, u) in urls.iter().enumerate() {
        if !allow(u, false) || (i == n && !allow(u, true)) {
            return Err(RedirectBlocked::of(u));
        }
    }
    Ok(())
}

//! XPC service scaffolding (ADR 0146 §4, Bozniak's shared-package rule 3).
//!
//! A shared package with code (phase 2: `xpc:voice`, FluidAudio inference for echo and nano) runs as a sandboxed
//! XPC service with no TCC grants and no network. This module is the platform-neutral half:
//!
//! - [`admit`]: a connection is accepted only from a peer whose code signature satisfies its module's designated
//!   requirement (checked on the audit token, never the pid), whose signed manifest was verified, and whose
//!   manifest declares this service as a dependency. Everything else is refused, so the service can't be used as
//!   a confused deputy.
//! - [`Sessions`]: state is per connection. A session opened on one connection is invisible to every other one
//!   (echo's audio and results never reach nano's connection), and closing a connection drops its sessions.
//! - [`Request`] / [`Reply`]: the message envelope, size-capped, methods from the service's allowlist.
//!
//! The macOS binding (`NSXPCConnection` with `setCodeSigningRequirement`, macOS 13+, plus the audit-token check)
//! comes with the first real service; [`Transport`] is what it implements. The confinement track's gate (f) drives
//! [`admit`] and [`Sessions`] with an echo and a nano peer.
use crate::manifest::{designated_requirement, DependencyKind, ModuleManifest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const ENVELOPE_VERSION: u32 = 1;
/// The voice service's LaunchAgent mach service (ADR 0144 §5). The Mac gate and dev builds override it with
/// `QRTZ_VOICE_SERVICE`; the platform binding reads the variable and passes it to [`mach_service_name`].
pub const VOICE_MACH_SERVICE: &str = "ai.qrtz.inference.voice";
pub const VOICE_SERVICE_ENV: &str = "QRTZ_VOICE_SERVICE";
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// A shared service as the shell knows it (from its own package's lockfile entry).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceDecl {
    /// `xpc:voice`.
    pub id: String,
    pub version: String,
    pub bundle_id: String,
    pub team_id: String,
    /// The LaunchAgent mach service name clients look up (defaults to `bundle_id`).
    pub mach_service: String,
    /// Methods the service answers; anything else is refused before it reaches the handler.
    pub methods: Vec<String>,
}

/// What the platform layer established about the connecting process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// The module the peer claims to be.
    pub module: String,
    /// The peer's code passed `SecCodeCheckValidity` against this requirement (from the audit token).
    pub satisfied_requirement: Option<String>,
    /// The peer module's installed manifest (signature verified at install, sha256 re-checked at launch).
    pub manifest: Option<ModuleManifest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmitError {
    Unsigned,
    WrongRequirement,
    NoVerifiedManifest,
    /// The manifest doesn't list this service (at this exact version).
    Undeclared,
    TeamMismatch,
}

/// Rule 3: only a signed module whose manifest declares this exact service version gets a connection.
pub fn admit(service: &ServiceDecl, peer: &Peer) -> Result<(), AdmitError> {
    let Some(m) = &peer.manifest else {
        return Err(AdmitError::NoVerifiedManifest);
    };
    let Some(got) = &peer.satisfied_requirement else {
        return Err(AdmitError::Unsigned);
    };
    if m.id != peer.module || *got != designated_requirement(&m.code.bundle_id, &m.code.team_id) {
        return Err(AdmitError::WrongRequirement);
    }
    if m.code.team_id != service.team_id {
        return Err(AdmitError::TeamMismatch);
    }
    let declared = m.dependencies.iter().any(|d| {
        d.kind == DependencyKind::XpcService
            && d.id == service.id
            && d.version == service.version
            && d.bundle_id.as_deref() == Some(service.bundle_id.as_str())
    });
    if !declared {
        return Err(AdmitError::Undeclared);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

/// Per-connection state. Lookups always name the connection; a session id from another connection finds nothing.
pub struct Sessions<S> {
    next_conn: u64,
    next_session: u64,
    conns: BTreeMap<ConnId, (String, BTreeMap<SessionId, S>)>,
}

impl<S> Default for Sessions<S> {
    fn default() -> Self {
        Self {
            next_conn: 1,
            next_session: 1,
            conns: BTreeMap::new(),
        }
    }
}

impl<S> Sessions<S> {
    /// Admit and register a connection.
    pub fn connect(&mut self, service: &ServiceDecl, peer: &Peer) -> Result<ConnId, AdmitError> {
        admit(service, peer)?;
        let id = ConnId(self.next_conn);
        self.next_conn += 1;
        self.conns.insert(id, (peer.module.clone(), BTreeMap::new()));
        Ok(id)
    }
    pub fn module(&self, conn: ConnId) -> Option<&str> {
        self.conns.get(&conn).map(|(m, _)| m.as_str())
    }
    pub fn open(&mut self, conn: ConnId, state: S) -> Option<SessionId> {
        let (_, sessions) = self.conns.get_mut(&conn)?;
        let id = SessionId(self.next_session);
        self.next_session += 1;
        sessions.insert(id, state);
        Some(id)
    }
    pub fn get(&self, conn: ConnId, session: SessionId) -> Option<&S> {
        self.conns.get(&conn)?.1.get(&session)
    }
    pub fn get_mut(&mut self, conn: ConnId, session: SessionId) -> Option<&mut S> {
        self.conns.get_mut(&conn)?.1.get_mut(&session)
    }
    pub fn end(&mut self, conn: ConnId, session: SessionId) -> Option<S> {
        self.conns.get_mut(&conn)?.1.remove(&session)
    }
    /// Connection gone (peer exit, invalidation): all its state goes with it.
    pub fn disconnect(&mut self, conn: ConnId) -> usize {
        self.conns.remove(&conn).map_or(0, |(_, s)| s.len())
    }
    pub fn session_count(&self, conn: ConnId) -> usize {
        self.conns.get(&conn).map_or(0, |(_, s)| s.len())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub v: u32,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub v: u32,
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    pub fn ok(id: u64, v: Value) -> Self {
        Self {
            v: ENVELOPE_VERSION,
            id,
            ok: Some(v),
            error: None,
        }
    }
    pub fn err(id: u64, e: impl Into<String>) -> Self {
        Self {
            v: ENVELOPE_VERSION,
            id,
            ok: None,
            error: Some(e.into()),
        }
    }
}

/// Decode one request: size cap, envelope version, method allowlist.
pub fn decode_request(service: &ServiceDecl, bytes: &[u8]) -> Result<Request, String> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("message too large".into());
    }
    let r: Request = serde_json::from_slice(bytes).map_err(|e| format!("bad request: {e}"))?;
    if r.v != ENVELOPE_VERSION {
        return Err(format!("unsupported envelope v{}", r.v));
    }
    if !service.methods.contains(&r.method) {
        return Err(format!("unknown method {}", r.method));
    }
    Ok(r)
}

/// The mach service name for a service bundle id. An override must still be one of ours (`ai.qrtz.`), so an
/// environment variable can't point a module at another vendor's service.
pub fn mach_service_name(bundle_id: &str, env_override: Option<&str>) -> String {
    match env_override.map(str::trim) {
        Some(o) if o.starts_with("ai.qrtz.") && o.len() <= 128 && !o.contains(char::is_whitespace) => {
            o.to_owned()
        }
        _ => bundle_id.to_owned(),
    }
}

/// What a platform binding provides: deliver bytes to one admitted connection.
pub trait Transport {
    fn send(&self, conn: ConnId, bytes: &[u8]) -> Result<(), String>;
}

/// In-process transport for tests and the Linux build: records what each connection was sent.
#[derive(Default)]
pub struct MemoryTransport {
    pub sent: std::sync::Mutex<Vec<(ConnId, Vec<u8>)>>,
}

impl Transport for MemoryTransport {
    fn send(&self, conn: ConnId, bytes: &[u8]) -> Result<(), String> {
        self.sent
            .lock()
            .map_err(|e| e.to_string())?
            .push((conn, bytes.to_vec()));
        Ok(())
    }
}

impl MemoryTransport {
    pub fn sent_to(&self, conn: ConnId) -> Vec<Vec<u8>> {
        self.sent
            .lock()
            .map(|s| {
                s.iter()
                    .filter(|(c, _)| *c == conn)
                    .map(|(_, b)| b.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

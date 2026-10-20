//! qrtz-helper-bridge: the Swift `qrtz-helper` stays a separate child process speaking its unchanged
//! JSON-lines protocol. This crate spawns it (`process`), supervises it (`supervisor`) and validates lines
//! against the zod-generated schema (`protocol`). Lifecycle-facing mapping lives in the app layer.
pub mod fdpass;
#[cfg(target_os = "macos")] // only the macOS suspended-spawn path locks a std Mutex
mod poison;
pub mod process;
pub mod protocol;
pub mod supervisor;
pub mod suspended;

pub use process::ProcessSpawn;
pub use protocol::{encode_host_message, message_type, parse_helper_line, protocol, ProtocolError};
pub use supervisor::{
    backoff_delay, ChildIo, ExitInfo, HelperState, HelperStatus, Spawn, Supervisor, SupervisorOptions,
};

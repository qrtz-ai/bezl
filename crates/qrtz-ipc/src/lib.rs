//! qrtz IPC for the Tauri build: the zod contract (embedded JSON), a validated router with per-sub-app
//! capability scoping, and event subscription rules. No Tauri dependency, so it is unit-testable.
#![forbid(unsafe_code)]
pub mod contract;
pub mod error;
pub mod router;
pub mod subscriptions;

pub use contract::{contract, idle_response, samples, Contract};
pub use error::{AppError, ErrorCode};
pub use router::{no_handler, CallContext, Handler, Router};
pub use subscriptions::{is_stream, validate_event, validate_event_paths, Streams, Subscriptions};

//! qrtz IPC for the Tauri build: the zod contract (embedded JSON), a validated router with per-sub-app
//! capability scoping, and event subscription rules. No Tauri dependency, so it is unit-testable.
pub mod contract;
pub mod error;
pub mod router;
pub mod subscriptions;

pub use contract::{contract, idle_response, samples, Contract};
pub use error::{AppError, ErrorCode};
pub use router::{no_handler, CallContext, Handler, Router};
pub use subscriptions::{validate_event, Subscriptions};

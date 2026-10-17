//! The recording runtime (feature `runtime`): the `NoteService` port plus the ports the shell injects.

pub mod api;
pub mod service;
pub mod steps;
pub mod store;

pub use api::*;
pub use service::{
    inside_dir, meeting_audio, utc_recording_title, NoteRuntime, PendingDeletes, RuntimeDeps,
    DELETE_WHILE_RECORDING,
};
pub use steps::{finalize_sync, CoreSteps};
pub use store::{lifecycle_error, DbExec, InlineDb, Store};

//! QRT-288: recover a poisoned std lock instead of panicking, and say so once (warn, no data).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::PoisonError;

static WARNED: AtomicBool = AtomicBool::new(false);

/// For `lock().unwrap_or_else(poison::recover)`: a thread panicked while holding the guard. The data it
/// protects stays usable at these call sites, so keep going; the first recovery in this crate logs a warning.
pub(crate) fn recover<T>(e: PoisonError<T>) -> T {
    if !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!("recovered a poisoned lock: a thread panicked while holding it");
    }
    e.into_inner()
}

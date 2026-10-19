//! ADR 0184: the first-run permissions onboarding. `onboarding.done` in config.json is the host's to write: the
//! shell's `onboarding:finish` (continue or skip) sets it, and a renderer's `config:set` that touches `onboarding`
//! is refused, the same way `modules.<id>.installed` is (ADR 0169, [`crate::installed::patch_touches_installed`]).
use serde_json::{json, Value};

/// True if a renderer's `config:set` patch touches `onboarding` (only the host writes it).
pub fn patch_touches_onboarding(patch: &Value) -> bool {
    patch.get("onboarding").is_some()
}

/// The config patch `onboarding:finish` writes.
pub fn finish_patch() -> Value {
    json!({"onboarding": {"done": true}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_host_writes_onboarding() {
        assert!(patch_touches_onboarding(&json!({"onboarding": {"done": true}})));
        assert!(patch_touches_onboarding(&json!({"onboarding": {"done": false}})));
        assert!(patch_touches_onboarding(&json!({"onboarding": {}})));
        assert!(patch_touches_onboarding(&json!({"onboarding": null})));
        assert!(!patch_touches_onboarding(&json!({"theme": "dark"})));
        assert!(!patch_touches_onboarding(
            &json!({"modules": {"echo": {"enabled": true}}})
        ));
        assert!(!patch_touches_onboarding(&json!({})));
    }

    #[test]
    fn finish_writes_done() {
        assert_eq!(finish_patch()["onboarding"]["done"], json!(true));
    }
}

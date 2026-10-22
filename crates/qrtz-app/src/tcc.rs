//! qrtz's own macOS permissions, read and asked from qrtz-app itself (Jared, 6:18 PM: "a user should be able to do
//! it"). qrtz-app is the responsible process for every helper it spawns, so its TCC rows are the ones qrtz-snap,
//! the speech helper and ctrl run under; asking from here makes macOS add "qrtz" to the list and show its own
//! prompt. Reads are cheap and live (no caching), so the UI can poll them every second while a prompt is showing.
//!
//! Accessibility: `AXIsProcessTrusted` / `AXIsProcessTrustedWithOptions({prompt: true})`.
//! Screen recording: `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess` (a grant reaches a running
//! process only after a restart, so the UI offers "restart qrtz" for it).
//! Input monitoring: `CGPreflightListenEventAccess` / `CGRequestListenEventAccess` (QRT-337: asked from here, not
//! from qrtz-snap, so the prompt and the list entry are qrtz's; a running tap only sees the grant after qrtz-snap
//! restarts, which the service does when the read flips).
//! Microphone stays with the helper (AVFoundation); the others have no preflight and are opened in System Settings.
use serde_json::Value;

/// System Settings → Privacy & Security, at the pane for `pane` (the permission ids the UI uses).
pub fn settings_url(pane: &str) -> Option<&'static str> {
    Some(match pane {
        "accessibility" => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        "screen-recording" => "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        "microphone" => "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        "input-monitoring" => "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
        "automation" => "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation",
        "files-and-folders" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders"
        }
        _ => return None,
    })
}

/// A grant that a running qrtz only sees after it restarts.
pub fn needs_restart(pane: &str) -> bool {
    matches!(pane, "screen-recording" | "input-monitoring")
}

/// The permissions this process can read itself (macOS only): accessibility, screen recording, input monitoring.
pub fn native_status() -> Vec<(&'static str, bool)> {
    #[cfg(target_os = "macos")]
    {
        vec![
            ("accessibility", mac::ax_trusted()),
            ("screen-recording", mac::screen_preflight()),
            ("input-monitoring", mac::listen_preflight()),
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        vec![]
    }
}

/// Puts the live native reads over a `PermissionsStatus` (from the helper, or the idle sample): the helper reported
/// once, possibly before the user switched qrtz on, and it runs under this same TCC identity, so the live read wins.
/// Permissions not read here keep the helper's answer.
pub fn overlay(v: &mut Value, live: &[(&str, bool)]) {
    if !v.is_object() {
        *v = Value::Object(Default::default());
    }
    for (k, on) in live {
        if *on {
            v[*k] = Value::from("granted");
        } else if v[*k].as_str() != Some("denied") {
            // TCC has no "denied" read for these two: not trusted is "not determined" until the user acts.
            v[*k] = Value::from("not-determined");
        }
    }
}

/// Accessibility as qrtz-app sees it right now.
pub fn ax_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::ax_trusted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Ask for `pane` the way macOS wants it asked: the system prompt (which also adds qrtz to the list) for
/// accessibility and screen recording, then the exact settings pane if it still isn't on. Returns whether the
/// settings pane was opened. Off macOS nothing happens.
pub fn request(pane: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let granted = match pane {
            "accessibility" => mac::ax_prompt(),
            "screen-recording" => mac::screen_request(),
            "input-monitoring" => mac::listen_request(),
            _ => false,
        };
        if granted {
            return false;
        }
        open_settings(pane)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pane;
        false
    }
}

/// The system Accessibility prompt, at most once per launch (no settings pane): for a module that is on and can't
/// work without it. Answers whether it asked now.
pub fn ask_accessibility_once() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ASKED: AtomicBool = AtomicBool::new(false);
    if ASKED.swap(true, Ordering::SeqCst) {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        tracing::info!("accessibility: asking macOS for qrtz (snap is on)");
        mac::ax_prompt();
    }
    true
}

/// Input monitoring as qrtz-app sees it right now (QRT-337).
pub fn listen_granted() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::listen_preflight()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The system Input Monitoring prompt, at most once per launch (QRT-337): summon is on and qrtz can't listen for
/// its chord. Asked by qrtz-app so macOS adds "qrtz" to the list. Answers whether it asked now.
pub fn ask_input_monitoring_once() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ASKED: AtomicBool = AtomicBool::new(false);
    if ASKED.swap(true, Ordering::SeqCst) {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        tracing::info!("input monitoring: asking macOS for qrtz (summon is on)");
        mac::listen_request();
    }
    true
}

/// Opens System Settings at `pane`. False for an unknown pane or off macOS.
pub fn open_settings(pane: &str) -> bool {
    let Some(url) = settings_url(pane) else {
        return false;
    };
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg(url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        false
    }
}

#[cfg(target_os = "macos")]
#[allow(non_upper_case_globals)]
mod mac {
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> u8;
        static kAXTrustedCheckOptionPrompt: CFTypeRef;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: CFTypeRef;
        // Opaque here: only their addresses are passed.
        static kCFTypeDictionaryKeyCallBacks: [usize; 6];
        static kCFTypeDictionaryValueCallBacks: [usize; 5];
        fn CFDictionaryCreate(
            allocator: CFTypeRef,
            keys: *const CFTypeRef,
            values: *const CFTypeRef,
            num: isize,
            key_cb: *const c_void,
            value_cb: *const c_void,
        ) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
    }

    pub fn ax_trusted() -> bool {
        // SAFETY: ApplicationServices call with no arguments or preconditions.
        unsafe { AXIsProcessTrusted() != 0 }
    }

    /// `AXIsProcessTrustedWithOptions({kAXTrustedCheckOptionPrompt: true})`: macOS adds qrtz to the Accessibility
    /// list and shows its own "qrtz would like to control this computer" prompt (once).
    pub fn ax_prompt() -> bool {
        // SAFETY: keys/values are 1-element arrays alive for the CFDictionaryCreate call; the callbacks are the
        // system's static CFType callbacks; `opts` is checked for null and released exactly once.
        unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let values = [kCFBooleanTrue];
            let opts = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(),
                std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast(),
            );
            if opts.is_null() {
                return AXIsProcessTrusted() != 0;
            }
            let ok = AXIsProcessTrustedWithOptions(opts) != 0;
            CFRelease(opts);
            ok
        }
    }

    pub fn screen_preflight() -> bool {
        // SAFETY: CoreGraphics call with no arguments or preconditions.
        unsafe { CGPreflightScreenCaptureAccess() }
    }

    pub fn screen_request() -> bool {
        // SAFETY: CoreGraphics call with no arguments or preconditions.
        unsafe { CGRequestScreenCaptureAccess() }
    }

    pub fn listen_preflight() -> bool {
        // SAFETY: CoreGraphics call with no arguments or preconditions.
        unsafe { CGPreflightListenEventAccess() }
    }

    /// Shows macOS's own Input Monitoring prompt (once) and adds qrtz to the list.
    pub fn listen_request() -> bool {
        // SAFETY: CoreGraphics call with no arguments or preconditions.
        unsafe { CGRequestListenEventAccess() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_pane_opens_its_own_privacy_anchor() {
        for (p, anchor) in [
            ("accessibility", "Privacy_Accessibility"),
            ("screen-recording", "Privacy_ScreenCapture"),
            ("microphone", "Privacy_Microphone"),
            ("input-monitoring", "Privacy_ListenEvent"),
            ("automation", "Privacy_Automation"),
            ("files-and-folders", "Privacy_FilesAndFolders"),
        ] {
            let u = settings_url(p).unwrap();
            assert!(
                u.starts_with("x-apple.systempreferences:com.apple.preference.security?"),
                "{u}"
            );
            assert!(u.ends_with(anchor), "{p}: {u}");
        }
        assert_eq!(settings_url("camera"), None);
        assert!(!open_settings("nope"));
    }

    #[test]
    fn input_monitoring_needs_a_restart_and_is_asked_once() {
        assert!(needs_restart("input-monitoring"));
        assert!(needs_restart("screen-recording"));
        assert!(!needs_restart("accessibility"));
        assert!(ask_input_monitoring_once());
        assert!(!ask_input_monitoring_once());
        #[cfg(not(target_os = "macos"))]
        assert!(!listen_granted());
    }

    #[test]
    fn the_automatic_prompt_comes_once_per_launch() {
        assert!(ask_accessibility_once());
        assert!(!ask_accessibility_once());
    }

    #[test]
    fn only_screen_recording_needs_a_restart() {
        assert!(needs_restart("screen-recording"));
        assert!(!needs_restart("accessibility"));
        assert!(!needs_restart("microphone"));
    }

    #[test]
    fn the_live_read_wins_over_a_stale_helper_report() {
        let mut v = json!({"accessibility": "not-determined", "screen-recording": "granted", "microphone": "granted"});
        overlay(&mut v, &[("accessibility", true), ("screen-recording", false)]);
        assert_eq!(
            v["accessibility"], "granted",
            "granted in system settings after the helper reported"
        );
        assert_eq!(v["screen-recording"], "not-determined", "switched off since");
        assert_eq!(
            v["microphone"], "granted",
            "not read here: the helper's answer stands"
        );
        let mut d = json!({"accessibility": "denied"});
        overlay(&mut d, &[("accessibility", false)]);
        assert_eq!(d["accessibility"], "denied");
        let mut n = Value::Null;
        overlay(&mut n, &[("accessibility", true)]);
        assert_eq!(n, json!({"accessibility": "granted"}));
    }
}

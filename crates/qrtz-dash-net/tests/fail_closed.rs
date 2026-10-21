//! Bozniak call 2: in a build without the dev override (release, or no dev-local), QRTZ_DASH_ORIGIN set at all
//! makes the helper refuse to start with a clear error. CI runs this with --release.
#![cfg(not(all(debug_assertions, feature = "dev-local")))]
use std::io::Write;
use std::process::{Command, Stdio};

fn run(origin: Option<&str>) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_qrtz-dash-net"));
    c.env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(o) = origin {
        c.env(qrtz_dash_net::DEV_ORIGIN_VAR, o);
    }
    let mut child = c.spawn().unwrap();
    // One harmless line (an unknown channel), then EOF.
    let res = child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"id\":1,\"channel\":\"dash:nope\",\"capability\":\"dash.read\"}\n");
    match res {
        Ok(()) => {}
        // A helper that refuses to start can exit before we write; the refusal itself is asserted by
        // the caller via exit status, stdout and stderr. A helper that should start must accept input.
        Err(e) if origin.is_some() && e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => panic!("writing helper stdin: {e}"),
    }
    child.wait_with_output().unwrap()
}

#[test]
fn origin_override_set_in_a_build_without_it_refuses_to_start() {
    for o in ["http://127.0.0.1:8799", "https://dash.qrtz.ai", "", "x"] {
        let out = run(Some(o));
        assert_eq!(out.status.code(), Some(2), "{o:?}");
        assert!(out.stdout.is_empty(), "{o:?}: answered before refusing");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("refusing to start") && err.contains("QRTZ_DASH_ORIGIN"),
            "{err}"
        );
    }
}

#[test]
fn without_the_variable_it_starts_and_answers() {
    let out = run(None);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"id\":1"));
}

//! ADR 0144 §2.3 / gate (f), Linux half: only declared, signed clients connect to a shared service, and state is
//! per connection.
mod common;
use common::*;
use qrtz_modules::ipc::{
    admit, decode_request, AdmitError, MemoryTransport, Peer, Reply, ServiceDecl, Sessions, Transport,
};
use qrtz_modules::manifest::designated_requirement;

fn voice() -> ServiceDecl {
    ServiceDecl {
        id: "xpc:voice".into(),
        version: "1.4.0".into(),
        bundle_id: "ai.qrtz.inference.voice".into(),
        team_id: TEAM.into(),
        mach_service: qrtz_modules::ipc::VOICE_MACH_SERVICE.into(),
        methods: vec![
            "session.open".into(),
            "session.feed".into(),
            "session.result".into(),
        ],
    }
}

fn peer(id: &str, deps: Vec<serde_json::Value>) -> Peer {
    Peer {
        module: id.into(),
        satisfied_requirement: Some(designated_requirement(&format!("ai.qrtz.{id}"), TEAM)),
        manifest: Some(manifest(id, "1.0.0", deps)),
    }
}

#[test]
fn only_a_signed_module_that_declares_the_exact_service_connects() {
    let s = voice();
    assert_eq!(admit(&s, &peer("echo", vec![xpc_dep("voice", "1.4.0")])), Ok(()));
    assert_eq!(admit(&s, &peer("ctrl", vec![])), Err(AdmitError::Undeclared));
    assert_eq!(
        admit(&s, &peer("echo", vec![xpc_dep("voice", "1.5.0")])),
        Err(AdmitError::Undeclared)
    );
    let mut unsigned = peer("echo", vec![xpc_dep("voice", "1.4.0")]);
    unsigned.satisfied_requirement = None;
    assert_eq!(admit(&s, &unsigned), Err(AdmitError::Unsigned));
    let mut impostor = peer("echo", vec![xpc_dep("voice", "1.4.0")]);
    impostor.module = "nano".into();
    assert_eq!(admit(&s, &impostor), Err(AdmitError::WrongRequirement));
    let mut other_team = peer("echo", vec![xpc_dep("voice", "1.4.0")]);
    other_team.satisfied_requirement = Some(designated_requirement("ai.qrtz.echo", "ZZZZZ99999"));
    assert_eq!(admit(&s, &other_team), Err(AdmitError::WrongRequirement));
    let mut no_manifest = peer("echo", vec![]);
    no_manifest.manifest = None;
    assert_eq!(admit(&s, &no_manifest), Err(AdmitError::NoVerifiedManifest));
}

#[test]
fn nanos_connection_cannot_read_an_in_flight_echo_session() {
    let s = voice();
    let mut t: Sessions<Vec<u8>> = Sessions::default();
    let echo = t
        .connect(&s, &peer("echo", vec![xpc_dep("voice", "1.4.0")]))
        .unwrap();
    let nano = t
        .connect(&s, &peer("nano", vec![xpc_dep("voice", "1.4.0")]))
        .unwrap();
    let dictation = t.open(echo, b"echo audio".to_vec()).unwrap();
    assert_eq!(
        t.get(echo, dictation).map(Vec::as_slice),
        Some(&b"echo audio"[..])
    );
    assert!(t.get(nano, dictation).is_none());
    assert!(t.get_mut(nano, dictation).is_none());
    assert!(t.end(nano, dictation).is_none());
    assert_eq!(t.session_count(echo), 1, "nano's end didn't touch echo's session");
    assert_eq!(t.disconnect(echo), 1);
    assert!(t.get(echo, dictation).is_none());
    // Refused peers never get a connection id.
    assert!(t.connect(&s, &peer("ctrl", vec![])).is_err());
    // Replies go only to the connection that asked.
    let tx = MemoryTransport::default();
    tx.send(
        nano,
        &serde_json::to_vec(&Reply::ok(1, serde_json::json!("x"))).unwrap(),
    )
    .unwrap();
    assert!(tx.sent_to(echo).is_empty());
    assert_eq!(tx.sent_to(nano).len(), 1);
}

#[test]
fn requests_are_capped_versioned_and_allowlisted() {
    let s = voice();
    assert!(decode_request(&s, br#"{"v":1,"id":1,"method":"session.open","params":{}}"#).is_ok());
    assert!(decode_request(&s, br#"{"v":2,"id":1,"method":"session.open"}"#).is_err());
    assert!(decode_request(&s, br#"{"v":1,"id":1,"method":"fs.read"}"#).is_err());
    assert!(decode_request(&s, br#"{"v":1,"id":1,"method":"session.open","x":1}"#).is_err());
    let big = vec![b' '; qrtz_modules::ipc::MAX_MESSAGE_BYTES + 1];
    assert!(decode_request(&s, &big).is_err());
}

#[test]
fn the_voice_mach_service_override_stays_ours() {
    use qrtz_modules::ipc::{mach_service_name, VOICE_MACH_SERVICE};
    assert_eq!(
        mach_service_name(VOICE_MACH_SERVICE, None),
        "ai.qrtz.inference.voice"
    );
    assert_eq!(
        mach_service_name(VOICE_MACH_SERVICE, Some("ai.qrtz.gate.inference-probe")),
        "ai.qrtz.gate.inference-probe"
    );
    for bad in ["com.evil.voice", "", "ai.qrtz.x y"] {
        assert_eq!(
            mach_service_name(VOICE_MACH_SERVICE, Some(bad)),
            VOICE_MACH_SERVICE,
            "{bad}"
        );
    }
}

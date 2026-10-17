//! Hardening cycle 4: which files a meeting delete may remove (N15). Same cases as TS `insideDir` /
//! `meetingAudio` in apps/desktop/src/main/note-service.hardening4.test.ts. The delete flow itself (N14, N16) is
//! pinned by the shared scenarios `N15-delete-removes-audio`, `N16-deleted-mid-backoff` and `F14-*`.

use qrtz_lifecycle::runtime::{inside_dir, meeting_audio};

#[test]
fn inside_dir_rejects_traversal_siblings_and_the_dir_itself() {
    let d = tempfile::tempdir().unwrap();
    let d = d.path();
    assert!(inside_dir(d, &d.join("a.wav")));
    assert!(inside_dir(d, &d.join("sub").join("a.wav")));
    assert!(!inside_dir(d, d));
    assert!(!inside_dir(d, &d.join("..").join("a.wav")));
    assert!(!inside_dir(d, &d.join("sub").join("..").join("..").join("a.wav")));
    let sibling = format!("{}2/a.wav", d.display());
    assert!(!inside_dir(d, std::path::Path::new(&sibling)));
    assert!(inside_dir(d, &d.join("..notes.wav")));
    assert!(!inside_dir(d, std::path::Path::new("relative.wav")));
    assert!(!inside_dir(d, std::path::Path::new("/etc/hosts")));
}

#[test]
fn meeting_audio_finds_own_recording_and_parts_only() {
    let d = tempfile::tempdir().unwrap();
    for f in ["x.wav", "x.p3.wav", "x.p.wav", "xy.wav", "x.p1.txt"] {
        std::fs::write(d.path().join(f), b"").unwrap();
    }
    let mut got: Vec<String> = meeting_audio(d.path(), "x", &[])
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    got.sort();
    assert_eq!(got, vec!["x.p3.wav", "x.wav"]);
    let listed = vec![d.path().join("x.wav").to_string_lossy().into_owned()];
    assert_eq!(meeting_audio(d.path(), "x", &listed).len(), 2, "deduplicated");
    assert!(meeting_audio(&d.path().join("missing"), "x", &[]).is_empty());
}

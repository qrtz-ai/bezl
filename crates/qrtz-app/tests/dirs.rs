//! N25: booting over a fresh or an older (0755) data dir under umask 022 leaves the app data dir and the
//! recordings dir 0700, and the config the service writes there is 0600. Own test binary: umask is per process.
#![cfg(unix)]
use qrtz_app::{AppDirs, ConfigStore};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn boot_makes_data_and_recordings_dirs_0700_under_umask_022() {
    unsafe { libc::umask(0o022) };
    let root = tempfile::tempdir().unwrap();

    // Fresh install: nothing exists yet (also a missing parent).
    let dirs = AppDirs::prepare(root.path().join("Application Support").join("qrtz")).unwrap();
    assert_eq!(dirs.recordings, dirs.data.join("recordings"));
    assert_eq!(mode(&dirs.data), 0o700);
    assert_eq!(mode(&dirs.recordings), 0o700);
    let mut cfg = ConfigStore::open(dirs.data.join("config.json"));
    cfg.set(&json!({"note": {"autoRecord": true}})).unwrap();
    assert_eq!(mode(&dirs.data.join("config.json")), 0o600);
    assert_eq!(
        mode(&dirs.data),
        0o700,
        "writing the config keeps the dir owner-only"
    );

    // An older build left both at the umask default: tightened on the next boot, contents untouched.
    let old = root.path().join("old");
    std::fs::create_dir_all(old.join("recordings")).unwrap();
    std::fs::write(old.join("recordings").join("m.wav"), b"RIFF").unwrap();
    assert_eq!(mode(&old), 0o755);
    let dirs = AppDirs::prepare(&old).unwrap();
    assert_eq!(mode(&dirs.data), 0o700);
    assert_eq!(mode(&dirs.recordings), 0o700);
    assert!(dirs.recordings.join("m.wav").exists());

    // The config store alone (its own create path) also makes its dir 0700.
    let lone = root.path().join("lone");
    let mut cfg = ConfigStore::open(lone.join("config.json"));
    cfg.set(&json!({"note": {"autoRecord": false}})).unwrap();
    assert_eq!(mode(&lone), 0o700);
}

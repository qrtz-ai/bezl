//! Bounded memory: a backup much larger than the allowed RSS growth exports and imports with flat memory.
//! Alone in its own test binary so the process high-water mark (VmHWM) measures only this test.
mod common;

use common::*;
use qrtz_backup::{export_backup, no_progress, ExportBackup};

fn hwm_kib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let l = s.lines().find(|l| l.starts_with("VmHWM:"))?;
    l.split_whitespace().nth(1)?.parse().ok()
}

fn reset_hwm() -> bool {
    std::fs::write("/proc/self/clear_refs", "5").is_ok()
}

#[test]
fn a_large_backup_streams_in_bounded_memory() {
    const MIB: usize = 1 << 20;
    let src = Lib::new();
    std::fs::create_dir_all(src.rec()).unwrap();
    // 3 recordings, 288 MiB total, written in 1 MiB chunks (never held whole).
    for (k, mib) in [(1u8, 160usize), (2, 96), (3, 32)] {
        use std::io::Write;
        let p = src.rec().join(format!("r{k}.wav"));
        let mut f = std::io::BufWriter::new(std::fs::File::create(&p).unwrap());
        let chunk: Vec<u8> = (0..MIB)
            .map(|i| (i as u8).wrapping_mul(k).wrapping_add(k))
            .collect();
        for _ in 0..mib {
            f.write_all(&chunk).unwrap();
        }
        let m: qrtz_storage::model::Meeting = serde_json::from_value(serde_json::json!({
            "id": format!("m{k}"), "title": "big", "platform": "zoom", "startedAt": k as i64, "status": "done",
            "participants": [], "audioPath": p.to_string_lossy(),
        }))
        .unwrap();
        src.db.notes().upsert_meeting(&m).unwrap();
    }
    let out = src.dir.path().join("big.zip");
    let can_measure = reset_hwm() && hwm_kib().is_some();
    let base = hwm_kib().unwrap_or(0);
    export_backup(
        &ExportBackup {
            db: &src.db,
            target: &out,
            recordings_dir: &src.rec(),
            temp_dir: &src.tmp(),
            app_version: "t",
            now_ms: 1,
        },
        &mut no_progress,
    )
    .unwrap();
    let after_export = hwm_kib().unwrap_or(0);
    let dst = Lib::new();
    let r = dst.import(&out).unwrap();
    assert_eq!((r.imported, r.audio_files), (3, 3));
    let after_import = hwm_kib().unwrap_or(0);
    for k in 1..=3 {
        let a = std::fs::metadata(src.rec().join(format!("r{k}.wav")))
            .unwrap()
            .len();
        let b = std::fs::metadata(dst.rec().join(format!("r{k}.wav")))
            .unwrap()
            .len();
        assert_eq!(a, b);
    }
    if can_measure {
        let grew = after_import.max(after_export).saturating_sub(base);
        eprintln!("RSS high-water growth: {grew} KiB for a 288 MiB backup");
        assert!(grew < 48 * 1024, "RSS grew {grew} KiB: not streaming");
    }
}

//! Slice 7 (ADR 0113): downloads cleanup end to end on Linux, through the `ctrl:*` channels against a temp
//! "Downloads" and "Trash". The default sorts by kind (moves only, no gate), never touches what's unknown, fresh,
//! partial, hidden or a folder, logs only hashed path refs, leaves the user's frontmost app, cursor and focus as
//! they were, and can be undone; trashing old installers waits at the gate with a `file` preview, deny moves
//! nothing, approve trashes exactly the previewed files, a new old installer asks again.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::files::{plan_cleanup, undo_ops, FileRoots, Planned};
use qrtz_ctrl::fs_exec::FsExecutor;
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::Route;
use qrtz_ctrl::runner::{Clock, Executor};
use qrtz_ctrl::schedule::IanaZone;
use qrtz_ctrl::service::CtrlService;
use serde_json::{json, Value};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const T0: u64 = 1_790_000_000_000;
const DAY: u64 = 24 * 3600 * 1000;

struct FakeClock(AtomicU64);
impl Clock for FakeClock {
    fn mono_ns(&self) -> u64 {
        self.0.fetch_add(1000, Ordering::SeqCst)
    }
    fn wall_ms(&self) -> u64 {
        T0
    }
}

fn at(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

/// A file modified `age_ms` before T0.
fn put(dir: &Path, name: &str, age_ms: u64) -> PathBuf {
    let p = dir.join(name);
    let f = File::create(&p).unwrap();
    f.set_modified(at(T0 - age_ms)).unwrap();
    p
}

struct Rig {
    s: CtrlService,
    fake: Arc<FakeHelper>,
    roots: FileRoots,
    _d: tempfile::TempDir,
}

fn rig() -> Rig {
    let d = tempfile::tempdir().unwrap();
    let roots = FileRoots {
        downloads: d.path().join("Downloads"),
        trash: d.path().join("Trash"),
        desktop: d.path().join("Desktop"),
    };
    fs::create_dir_all(&roots.downloads).unwrap();
    fs::create_dir_all(&roots.trash).unwrap();
    let fake = Arc::new(FakeHelper::new(FakeWorld::typical(1)));
    let s = CtrlService::open_with_zone(
        d.path().join("store"),
        Box::new(FakeClock(AtomicU64::new(0))),
        Box::new(IanaZone(Los_Angeles)),
    )
    .with_helper(Arc::new(HelperClient::new(Box::new(fake.clone()))))
    .with_files(roots.clone());
    Rig {
        s,
        fake,
        roots,
        _d: d,
    }
}

fn typical(r: &Rig) {
    let dl = &r.roots.downloads;
    put(dl, "Photo.PNG", 2 * DAY);
    put(dl, "taxes 2025.pdf", 3 * DAY);
    put(dl, "site.zip", 40 * DAY);
    put(dl, "OldApp.dmg", 45 * DAY);
    put(dl, "NewApp.dmg", 2 * DAY);
    put(dl, "notes", 5 * DAY);
    put(dl, ".DS_Store", 5 * DAY);
    put(dl, "movie.mp4.crdownload", 5 * DAY);
    put(dl, "just-now.png", 60 * 1000);
    fs::create_dir(dl.join("project")).unwrap();
}

fn listing(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in walk(dir) {
        out.push(e.strip_prefix(dir).unwrap().to_string_lossy().into_owned());
    }
    out.sort();
    out
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            v.push(p.clone());
            v.extend(walk(&p));
        } else {
            v.push(p);
        }
    }
    v
}

fn run(s: &CtrlService, text: &str) -> Value {
    let id = s
        .call(
            "ctrl:create-task",
            json!({"text": text, "schedule": {"kind": "weekly", "weekday": 0, "hour": 18, "minute": 0}}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = s.call("ctrl:run-now", json!({"id": id})).unwrap();
    s.call("ctrl:run", json!({"runId": r["id"]})).unwrap()
}

fn kinds(v: &Value) -> Vec<String> {
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect()
}

fn count(v: &Value, kind: &str) -> usize {
    kinds(v).iter().filter(|k| *k == kind).count()
}

fn untouched(w: &FakeWorld) {
    let t = FakeWorld::typical(0);
    assert_eq!(w.frontmost_pid, t.frontmost_pid, "frontmost app changed");
    assert_eq!(w.cursor, t.cursor, "cursor moved");
    assert_eq!(w.focused, t.focused, "focus changed");
}

/// The persisted run (what reaches disk) holds no file name or path.
fn no_names_on_disk(r: &Rig, names: &[&str]) {
    let store = r._d.path().join("store");
    for f in walk(&store) {
        if f.is_file() {
            let text = fs::read_to_string(&f).unwrap_or_default();
            for n in names {
                assert!(!text.contains(n), "{n} reached {}", f.display());
            }
            assert!(
                !text.contains(&*r.roots.downloads.to_string_lossy()),
                "a path reached the log"
            );
        }
    }
}

#[test]
fn weekly_downloads_cleanup_sorts_by_kind_without_asking_and_without_touching_the_screen() {
    let r = rig();
    typical(&r);
    let v = run(&r.s, "every week clean up my downloads folder");
    let st = &v["state"];
    assert_eq!(st["status"], "done", "{v}");
    assert_eq!(count(&v, "confirm.requested"), 0, "moves only: no gate");
    assert_eq!(count(&v, "action.requested"), 1, "one batch action");
    assert_eq!(st["violations"], 0);
    assert_eq!(
        listing(&r.roots.downloads),
        vec![
            ".DS_Store",
            "archives",
            "archives/site.zip",
            "documents",
            "documents/taxes 2025.pdf",
            "images",
            "images/Photo.PNG",
            "installers",
            "installers/NewApp.dmg",
            "installers/OldApp.dmg",
            "just-now.png",
            "movie.mp4.crdownload",
            "notes",
            "project",
        ]
    );
    assert!(listing(&r.roots.trash).is_empty(), "the default never trashes");
    // The verify re-read the folder (every planned file in place) and passed: the run is done, not asked.
    assert_eq!(count(&v, "verify.result"), 1);
    let disk = fs::read_to_string(
        walk(&r._d.path().join("store"))
            .into_iter()
            .find(|p| {
                p.is_file()
                    && fs::read_to_string(p)
                        .unwrap_or_default()
                        .contains("files_remaining")
            })
            .expect("the verify's check reached the run log"),
    )
    .unwrap();
    assert!(disk.contains("\"batch\""), "one batch op in the log");
    assert!(
        r.fake.world().acted.is_empty(),
        "no helper act: files never go through the UI"
    );
    untouched(&r.fake.world());
    no_names_on_disk(&r, &["Photo", "taxes", "site.zip", "OldApp", "NewApp"]);

    // A second run finds nothing left to tidy: done, no action.
    let v2 = run(&r.s, "every week clean up my downloads folder");
    assert_eq!(v2["state"]["status"], "done", "{v2}");
    assert_eq!(count(&v2, "action.requested"), 0);
}

#[test]
fn a_cleanup_is_undone_exactly_and_a_failed_batch_rolls_back() {
    let r = rig();
    typical(&r);
    let before = listing(&r.roots.downloads);
    let plan = plan_cleanup(&r.roots, "route.files.sort_by_kind", at(T0)).unwrap();
    assert_eq!(plan.files(), 5);
    let (op, store) = plan.to_op(&r.roots);
    let mut ex = FsExecutor {
        store,
        trash_dir: r.roots.trash.clone(),
    };
    ex.execute(&Route::Files(op.clone())).unwrap();
    assert_ne!(listing(&r.roots.downloads), before);
    ex.undo(&op).unwrap();
    assert_eq!(
        listing(&r.roots.downloads),
        before,
        "the inverse batch restores the folder"
    );

    // Redo, then the planned-path undo restores it too.
    ex.execute(&Route::Files(op.clone())).unwrap();
    assert!(undo_ops(&plan.ops, &r.roots).iter().all(Result::is_ok));
    assert_eq!(listing(&r.roots.downloads), before);

    // A name that appears in a kind folder between plan and act: that move fails, the whole batch rolls back.
    ex.execute(&Route::Files(op.clone())).unwrap();
    ex.undo(&op).unwrap();
    fs::create_dir(r.roots.downloads.join("images")).unwrap();
    File::create(r.roots.downloads.join("images/Photo.PNG")).unwrap();
    let mut b = before.clone();
    b.push("images".into());
    b.push("images/Photo.PNG".into());
    b.sort();
    let err = ex.execute(&Route::Files(op)).unwrap_err();
    assert!(err.starts_with("fs."), "{err}");
    assert_eq!(listing(&r.roots.downloads), b, "nothing half-done");
}

#[test]
fn trashing_old_installers_waits_at_the_gate_with_a_file_preview() {
    let r = rig();
    typical(&r);
    put(&r.roots.downloads, "Ancient.pkg", 90 * DAY);
    let v = run(&r.s, "every week delete old installers from my downloads");
    let st = &v["state"];
    assert_eq!(st["status"], "needs-confirm", "{v}");
    let items: Vec<(String, String)> = st["confirm"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["kind"].as_str().unwrap().to_string(),
                i["label"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        items,
        vec![
            ("file".to_string(), "ancient.pkg".to_string()),
            ("file".to_string(), "oldapp.dmg".to_string()),
        ],
        "only installers older than 30 days, nothing new"
    );
    assert_eq!(count(&v, "action.requested"), 0);
    no_names_on_disk(&r, &["Ancient", "OldApp"]);

    // Deny: nothing moves.
    let snapshot = listing(&r.roots.downloads);
    let gate = |v: &Value| json!({"runId": v["run"]["id"], "gateId": v["state"]["confirm"]["gateId"]});
    let mut a = gate(&v);
    a["answer"] = json!("deny");
    let denied = r.s.call("ctrl:confirm", a).unwrap();
    assert_ne!(denied["state"]["status"], "done", "{denied}");
    assert_eq!(listing(&r.roots.downloads), snapshot);
    assert!(listing(&r.roots.trash).is_empty());

    // A new run, then another old installer shows up before the answer: the re-check asks again, nothing trashed.
    let v = run(&r.s, "every week delete old installers from my downloads");
    put(&r.roots.downloads, "Stale.dmg", 60 * DAY);
    let mut a = gate(&v);
    a["answer"] = json!("approve");
    let grown = r.s.call("ctrl:confirm", a).unwrap();
    assert_eq!(grown["state"]["status"], "needs-confirm", "{grown}");
    assert!(listing(&r.roots.trash).is_empty());
    assert_eq!(grown["state"]["confirm"]["items"].as_array().unwrap().len(), 3);

    // Approve the fresh card: exactly those three go to the Trash, the rest stays, the screen is untouched.
    let mut a = gate(&grown);
    a["answer"] = json!("approve");
    let done = r.s.call("ctrl:confirm", a).unwrap();
    assert_eq!(done["state"]["status"], "done", "{done}");
    assert_eq!(
        listing(&r.roots.trash),
        vec!["Ancient.pkg", "OldApp.dmg", "Stale.dmg"]
    );
    assert!(r.roots.downloads.join("NewApp.dmg").exists());
    assert!(
        r.roots.downloads.join("Photo.PNG").exists(),
        "trash_old doesn't sort"
    );
    untouched(&r.fake.world());

    // Undo is a move back out of the Trash.
    let restored = undo_ops(
        &[Planned::Trash {
            src: r.roots.downloads.join("Stale.dmg"),
        }],
        &r.roots,
    );
    assert!(restored.iter().all(Result::is_ok));
    assert!(r.roots.downloads.join("Stale.dmg").exists());
}

//! ADR 0117: putting a finished downloads cleanup back, through the `ctrl:*` channels against a temp "Downloads"
//! and "Trash". Every file goes back where it was and the kind folders ctrl made go away; the put-back is an
//! `undo.done` event, so scrubbing and replay stay deterministic and no name reaches the disk. If anything moved
//! since (renamed, or its name is taken again in Downloads) ctrl refuses and says which files, nothing moves; only an
//! explicit "put back the rest" moves the others. A batch goes back once; a put-back that fails midway moves nothing.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::files::{plan_undo, run_undo, FileRoots, UndoPlan};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::runner::Clock;
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
        capture_prefs: None,
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

fn undo(r: &Rig, v: &Value, rest: bool) -> Value {
    r.s.call("ctrl:undo", json!({"runId": v["run"]["id"], "rest": rest}))
        .unwrap()
}

fn original() -> Vec<String> {
    [
        ".DS_Store",
        "NewApp.dmg",
        "OldApp.dmg",
        "Photo.PNG",
        "just-now.png",
        "movie.mp4.crdownload",
        "notes",
        "project",
        "site.zip",
        "taxes 2025.pdf",
    ]
    .map(String::from)
    .to_vec()
}

#[test]
fn a_finished_cleanup_goes_back_exactly_and_replays_the_same() {
    let r = rig();
    typical(&r);
    let v = run(&r.s, "every week clean up my downloads folder");
    assert_eq!(v["state"]["status"], "done", "{v}");
    let u = &v["state"]["undo"];
    assert_eq!(u["files"], 5, "{u}");
    assert_eq!(u["ready"], 5);
    assert!(u["cantUndo"].as_array().unwrap().is_empty());
    assert!(u["done"].is_null());

    let after = undo(&r, &v, false);
    assert_eq!(
        listing(&r.roots.downloads),
        original(),
        "every file back, kind folders gone"
    );
    assert!(listing(&r.roots.trash).is_empty());
    assert_eq!(count(&after, "undo.done"), 1);
    assert_eq!(after["state"]["undo"]["done"]["undone"], 5, "{after}");
    assert_eq!(after["state"]["status"], "done", "the run stays done");
    untouched(&r.fake.world());
    no_names_on_disk(&r, &["Photo", "taxes", "OldApp", "NewApp", "site.zip"]);

    // Scrub: before the undo event the batch is shown as logged (nothing judged), after it as put back.
    let n = after["events"].as_array().unwrap().len() as u64;
    let before =
        r.s.call("ctrl:run", json!({"runId": v["run"]["id"], "at": n - 1}))
            .unwrap();
    assert!(before["state"]["undo"]["done"].is_null(), "{before}");
    assert_eq!(
        before["state"]["undo"]["ready"], 0,
        "a scrubbed view reads nothing from disk"
    );
    let again = r.s.call("ctrl:run", json!({"runId": v["run"]["id"]})).unwrap();
    assert_eq!(
        again["state"], after["state"],
        "replay from the log is the same state"
    );

    // A batch goes back once.
    let e =
        r.s.call("ctrl:undo", json!({"runId": v["run"]["id"]}))
            .unwrap_err();
    assert!(e.message.contains("already"), "{}", e.message);
    assert_eq!(listing(&r.roots.downloads), original());
}

#[test]
fn a_changed_folder_refuses_and_says_what_cant_go_back_then_puts_back_the_rest_only_when_asked() {
    let r = rig();
    typical(&r);
    let v = run(&r.s, "every week clean up my downloads folder");
    let dl = &r.roots.downloads;
    // Since the run: the photo was renamed, and a new "taxes 2025.pdf" landed in Downloads.
    fs::rename(dl.join("images/Photo.PNG"), dl.join("images/beach.png")).unwrap();
    put(dl, "taxes 2025.pdf", DAY);

    let v = r.s.call("ctrl:run", json!({"runId": v["run"]["id"]})).unwrap();
    let u = &v["state"]["undo"];
    assert_eq!(u["files"], 5, "{u}");
    assert_eq!(u["ready"], 3);
    let cant: Vec<(String, String)> = u["cantUndo"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["label"].as_str().unwrap().into(),
                c["why"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        cant,
        vec![
            ("photo.png".to_string(), "moved".to_string()),
            ("taxes 2025.pdf".to_string(), "occupied".to_string()),
        ]
    );

    // Without "the rest": refused, logged, nothing moves.
    let snapshot = listing(dl);
    let refused = undo(&r, &v, false);
    assert_eq!(count(&refused, "undo.refused"), 1);
    assert_eq!(count(&refused, "undo.done"), 0);
    assert_eq!(listing(dl), snapshot);
    assert!(refused["state"]["undo"]["done"].is_null());

    // "put back the other 3": exactly those go back; the renamed photo and the occupied pdf stay where they are.
    let done = undo(&r, &refused, true);
    assert_eq!(done["state"]["undo"]["done"]["undone"], 3, "{done}");
    assert_eq!(done["state"]["undo"]["cantUndo"].as_array().unwrap().len(), 2);
    assert_eq!(
        listing(dl),
        vec![
            ".DS_Store",
            "NewApp.dmg",
            "OldApp.dmg",
            "documents",
            "documents/taxes 2025.pdf",
            "images",
            "images/beach.png",
            "just-now.png",
            "movie.mp4.crdownload",
            "notes",
            "project",
            "site.zip",
            "taxes 2025.pdf",
        ],
        "folders that still hold something stay"
    );
    untouched(&r.fake.world());
}

#[test]
fn trashed_installers_come_back_from_the_trash() {
    let r = rig();
    typical(&r);
    let v = run(&r.s, "every week delete old installers from my downloads");
    let a = json!({"runId": v["run"]["id"], "gateId": v["state"]["confirm"]["gateId"], "answer": "approve"});
    let done = r.s.call("ctrl:confirm", a).unwrap();
    assert_eq!(done["state"]["status"], "done", "{done}");
    assert_eq!(listing(&r.roots.trash), vec!["OldApp.dmg"]);
    assert_eq!(done["state"]["undo"]["ready"], 1, "{done}");
    let back = undo(&r, &done, false);
    assert_eq!(back["state"]["undo"]["done"]["undone"], 1);
    assert!(listing(&r.roots.trash).is_empty());
    assert!(r.roots.downloads.join("OldApp.dmg").exists());
}

#[test]
fn runs_with_nothing_moved_have_nothing_to_put_back() {
    let r = rig();
    let v = run(&r.s, "every week clean up my downloads folder");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(v["state"]["undo"].is_null());
    let e =
        r.s.call("ctrl:undo", json!({"runId": v["run"]["id"]}))
            .unwrap_err();
    assert!(e.message.contains("didn't move"), "{}", e.message);
}

#[test]
fn a_put_back_that_fails_midway_moves_nothing() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = (d.path().join("a"), d.path().join("b"));
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    File::create(a.join("one")).unwrap();
    File::create(a.join("two")).unwrap();
    let plan = UndoPlan {
        back: vec![(a.join("one"), b.join("one")), (a.join("two"), b.join("two"))],
        ..Default::default()
    };
    // The second target is taken after the plan was read.
    File::create(b.join("two")).unwrap();
    assert!(run_undo(&plan).is_err());
    assert!(
        a.join("one").exists() && a.join("two").exists(),
        "the first move was put back"
    );
    assert!(!b.join("one").exists());
    let roots = FileRoots {
        downloads: a.clone(),
        trash: b,
        desktop: d.path().join("Desktop"),
        capture_prefs: None,
    };
    assert_eq!(
        plan_undo(&qrtz_ctrl::ladder::FsOp::Batch { ops: vec![] }, &roots),
        UndoPlan::default()
    );
}

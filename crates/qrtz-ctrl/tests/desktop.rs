//! ADR 0124: "every week tidy my desktop" / "move my screenshots off the desktop" end to end on Linux, through the
//! `ctrl:*` channels against a temp "Desktop". Only the screenshots and screen recordings macOS saved there (its
//! own names and kinds) move, into `Desktop/screenshots`, in one batch, unattended (nothing is deleted), and the run
//! can be put back. Nothing else on the Desktop moves, Downloads isn't touched, nothing touches the screen, names
//! are hashed on disk, and the log replays. Trashing them happens only when the task says so, behind the gate.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::files::{capture_location, is_screen_capture, plan_desktop_screenshots, FileRoots, Planned};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::schedule::IanaZone;
use qrtz_ctrl::service::CtrlService;
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const T0: u64 = 1_790_000_000_000;
const DAY: u64 = 24 * 3600 * 1000;

struct FakeClock {
    wall: AtomicU64,
    mono: AtomicU64,
}
impl Clock for FakeClock {
    fn mono_ns(&self) -> u64 {
        self.mono.fetch_add(1000, Ordering::SeqCst)
    }
    fn wall_ms(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
}
struct ClockHandle(Arc<FakeClock>);
impl Clock for ClockHandle {
    fn mono_ns(&self) -> u64 {
        self.0.mono_ns()
    }
    fn wall_ms(&self) -> u64 {
        self.0.wall_ms()
    }
}

fn at(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

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
    d: tempfile::TempDir,
}

fn rig() -> Rig {
    rig_with(false)
}

/// With `prefs`, the roots read a Screenshot app preferences file at `<home>/prefs.plist` (ADR 0126).
fn rig_with(prefs: bool) -> Rig {
    let d = tempfile::tempdir().unwrap();
    let roots = FileRoots {
        downloads: d.path().join("Downloads"),
        trash: d.path().join("Trash"),
        desktop: d.path().join("Desktop"),
        capture_prefs: prefs.then(|| d.path().join("prefs.plist")),
    };
    for p in [&roots.downloads, &roots.trash, &roots.desktop] {
        fs::create_dir_all(p).unwrap();
    }
    let fake = Arc::new(FakeHelper::new(FakeWorld::typical(1)));
    let clock = Arc::new(FakeClock {
        wall: AtomicU64::new(T0),
        mono: AtomicU64::new(0),
    });
    let s = CtrlService::open_with_zone(
        d.path().join("store"),
        Box::new(ClockHandle(clock)),
        Box::new(IanaZone(Los_Angeles)),
    )
    .with_helper(Arc::new(HelperClient::new(Box::new(fake.clone()))))
    .with_files(roots.clone());
    Rig { s, fake, roots, d }
}

const SHOTS: [&str; 4] = [
    "Screen Recording 2026-10-12 at 9.02.11 AM.mov",
    "Screen Shot 2021-03-01 at 10.00.00 PM.png",
    "Screenshot 2026-10-14 at 6.37.12 PM (2).png",
    "Screenshot 2026-10-15 at 8.01.44 AM.png",
];

/// Four settled captures, one just taken, and things that only look like one.
fn typical(r: &Rig) {
    let dt = &r.roots.desktop;
    for n in SHOTS {
        put(dt, n, 2 * DAY);
    }
    put(dt, "Screenshot 2026-10-18 at 6.30.00 AM.png", 60 * 1000);
    put(dt, "screenshot ideas.txt", 3 * DAY);
    put(dt, "Screenshot 2026-10-10 at 1.00.00 PM.txt", 3 * DAY);
    put(dt, "Screen Recording notes.mov", 3 * DAY);
    put(dt, "taxes.pdf", 3 * DAY);
    fs::create_dir(dt.join("Screenshot 2026-01-01 folder")).unwrap();
    put(
        &r.roots.downloads,
        "Screenshot 2026-10-01 at 1.00.00 PM.png",
        3 * DAY,
    );
}

fn listing(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = walk(dir)
        .into_iter()
        .map(|e| e.strip_prefix(dir).unwrap().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() && !p.is_symlink() {
            v.push(p.clone());
            v.extend(walk(&p));
        } else {
            v.push(p);
        }
    }
    v
}

fn run(s: &CtrlService, text: &str) -> (String, Value) {
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
    let v = s.call("ctrl:run", json!({"runId": r["id"]})).unwrap();
    (id, v)
}

fn count(v: &Value, kind: &str) -> usize {
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == kind)
        .count()
}

fn untouched(r: &Rig) {
    let w = r.fake.world();
    let t = FakeWorld::typical(0);
    assert_eq!(w.frontmost_pid, t.frontmost_pid, "frontmost app changed");
    assert_eq!(w.cursor, t.cursor, "cursor moved");
    assert_eq!(w.focused, t.focused, "focus changed");
    assert!(w.acted.is_empty(), "no helper act: the Finder is never driven");
}

fn no_names_on_disk(r: &Rig) {
    for f in walk(&r.d.path().join("store")) {
        if f.is_file() {
            let text = fs::read_to_string(&f).unwrap_or_default().to_lowercase();
            for n in ["screenshot 20", "screen shot", "screen recording", "desktop"] {
                assert!(
                    !text.contains(n)
                        || n == "desktop"
                            && !text.contains(&*r.roots.desktop.to_string_lossy().to_lowercase()),
                    "{n} reached {}",
                    f.display()
                );
            }
        }
    }
}

fn after() -> Vec<String> {
    let mut v: Vec<String> = vec![
        "Screen Recording notes.mov".into(),
        "Screenshot 2026-01-01 folder".into(),
        "Screenshot 2026-10-10 at 1.00.00 PM.txt".into(),
        "Screenshot 2026-10-18 at 6.30.00 AM.png".into(),
        "screenshot ideas.txt".into(),
        "screenshots".into(),
        "taxes.pdf".into(),
    ];
    v.extend(SHOTS.iter().map(|n| format!("screenshots/{n}")));
    v.sort();
    v
}

#[test]
fn tidy_my_desktop_moves_only_the_screenshots_unattended_and_puts_them_back() {
    let r = rig();
    typical(&r);
    let before = listing(&r.roots.desktop);
    let downloads = listing(&r.roots.downloads);
    let (t, v) = run(&r.s, "every week tidy my desktop");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(count(&v, "confirm.requested"), 0, "nothing deleted: no gate");
    assert_eq!(count(&v, "action.requested"), 1, "one batch");
    assert_eq!(listing(&r.roots.desktop), after());
    assert_eq!(listing(&r.roots.downloads), downloads, "downloads untouched");
    assert!(listing(&r.roots.trash).is_empty(), "nothing trashed");
    untouched(&r);
    no_names_on_disk(&r);

    // The log replays and verifies.
    let log = Store::open(r.d.path().join("store"))
        .unwrap()
        .run(v["run"]["id"].as_str().unwrap())
        .unwrap();
    log.verify().unwrap();
    replay_check(
        &log,
        &Goal {
            task_id: &t,
            text: "every week tidy my desktop",
        },
    )
    .unwrap();

    // Put back (ADR 0117): every capture returns to the Desktop, the screenshots folder goes.
    let u = &v["state"]["undo"];
    assert_eq!(u["files"], 4, "{u}");
    assert_eq!(u["ready"], 4);
    let back =
        r.s.call("ctrl:undo", json!({"runId": v["run"]["id"], "rest": false}))
            .unwrap();
    assert_eq!(back["state"]["undo"]["done"]["undone"], 4, "{back}");
    assert_eq!(listing(&r.roots.desktop), before);
    untouched(&r);

    // The next week the same task does the same, and a second run with nothing left is just done.
    let (_, v) = run(&r.s, "move my screenshots off the desktop");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(listing(&r.roots.desktop), after());
    let (_, v) = run(&r.s, "move my screenshots off the desktop");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(count(&v, "action.requested"), 0, "nothing to move");
}

#[test]
fn trashing_the_screenshots_waits_at_the_gate_and_can_be_put_back() {
    let r = rig();
    typical(&r);
    let before = listing(&r.roots.desktop);
    let (_, v) = run(&r.s, "every week delete the screenshots on my desktop");
    assert_eq!(v["state"]["status"], "needs-confirm", "{v}");
    assert_eq!(v["state"]["confirm"]["move"], "move screenshots to the trash");
    let items: Vec<String> = v["state"]["confirm"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(items.len(), 4, "{items:?}");
    assert_eq!(
        listing(&r.roots.desktop),
        before,
        "nothing moves before the answer"
    );
    no_names_on_disk(&r);
    let done =
        r.s.call(
            "ctrl:confirm",
            json!({"runId": v["run"]["id"], "gateId": v["state"]["confirm"]["gateId"], "answer": "approve"}),
        )
        .unwrap();
    assert_eq!(done["state"]["status"], "done", "{done}");
    let mut trashed = listing(&r.roots.trash);
    trashed.sort();
    let mut want: Vec<String> = SHOTS.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(trashed, want);
    untouched(&r);
    let back =
        r.s.call("ctrl:undo", json!({"runId": v["run"]["id"], "rest": false}))
            .unwrap();
    assert_eq!(back["state"]["undo"]["done"]["undone"], 4, "{back}");
    assert_eq!(listing(&r.roots.desktop), before);
    assert!(listing(&r.roots.trash).is_empty());
}

#[test]
fn the_planner_matches_macos_capture_names_and_kinds_only() {
    for n in SHOTS {
        assert!(is_screen_capture(n), "{n}");
    }
    for n in [
        "screenshot ideas.txt",
        "Screenshot 2026-10-10 at 1.00.00 PM.txt",
        "Screen Recording 2026-10-10 at 1.00.00 PM.png",
        "Screenshot 2026-10-10 at 1.00.00 PM.mov",
        "Screen Recording notes.mov",
        "My Screenshot 2026-10-10.png",
        "Screenshot.png",
    ] {
        assert!(!is_screen_capture(n), "{n}");
    }
    let r = rig();
    typical(&r);
    // A same-named file already in the folder keeps the new one where it is; fresh ones wait 10 minutes.
    fs::create_dir(r.roots.desktop.join("screenshots")).unwrap();
    put(&r.roots.desktop.join("screenshots"), SHOTS[0], DAY);
    let p = plan_desktop_screenshots(&r.roots, false, at(T0)).unwrap();
    assert_eq!(p.files(), 3);
    assert!(!p.ops.iter().any(|o| matches!(o, Planned::MakeDir { .. })));
    let later = plan_desktop_screenshots(&r.roots, false, at(T0 + 11 * 60 * 1000)).unwrap();
    assert_eq!(later.files(), 4, "the just-taken screenshot is settled now");
    // Pure: the same listing and clock give the same plan.
    assert_eq!(p, plan_desktop_screenshots(&r.roots, false, at(T0)).unwrap());
    // A file named "screenshots" where the folder would go: nothing moves.
    fs::remove_dir_all(r.roots.desktop.join("screenshots")).unwrap();
    File::create(r.roots.desktop.join("screenshots")).unwrap();
    assert_eq!(
        plan_desktop_screenshots(&r.roots, false, at(T0)).unwrap().files(),
        0
    );
    // A missing Desktop is an error (the run asks), never a guess.
    let gone = FileRoots {
        desktop: r.d.path().join("nope"),
        ..r.roots.clone()
    };
    assert!(plan_desktop_screenshots(&gone, false, at(T0)).is_err());
}

#[test]
fn my_screenshots_without_naming_the_desktop_is_the_same_task_and_downloads_tidy_is_unchanged() {
    let r = rig();
    typical(&r);
    let (_, v) = run(&r.s, "every week clean up my screenshots");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(listing(&r.roots.desktop), after());
    // "clean up my downloads" still sorts Downloads by kind and never reaches into the Desktop.
    let (_, v) = run(&r.s, "every week clean up my downloads folder");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(listing(&r.roots.desktop), after());
    assert!(r.roots.downloads.join("images").is_dir());
    untouched(&r);
}

#[test]
fn tidy_my_documents_never_sorts_another_folder() {
    let r = rig();
    typical(&r);
    put(&r.roots.downloads, "photo.png", 3 * DAY);
    let downloads = listing(&r.roots.downloads);
    let desktop = listing(&r.roots.desktop);
    let (t, v) = run(&r.s, "every week tidy my documents");
    // ADR 0125: it stops with a named hold (no empty question left open), nothing moves.
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert_eq!(v["state"]["held"], "folder-not-yet", "{v}");
    assert!(v["state"]["ask"].is_null(), "{v}");
    assert_eq!(count(&v, "action.requested"), 0, "{v}");
    assert_eq!(count(&v, "exec.held"), 1, "{v}");
    assert_eq!(listing(&r.roots.downloads), downloads);
    assert_eq!(listing(&r.roots.desktop), desktop);
    untouched(&r);
    no_names_on_disk(&r);
    let log = Store::open(r.d.path().join("store"))
        .unwrap()
        .run(v["run"]["id"].as_str().unwrap())
        .unwrap();
    log.verify().unwrap();
    replay_check(
        &log,
        &Goal {
            task_id: &t,
            text: "every week tidy my documents",
        },
    )
    .unwrap();
    // The chosen move is the hold, labelled in lowercase; the run is not left asking.
    let mv = v["state"]["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["node"] == "choose_move")
        .unwrap()
        .clone();
    assert_eq!(mv["chosen"], "route.files.documents_hold", "{mv}");
    let shown = mv["chosenLabel"].as_str().unwrap();
    assert_eq!(shown, "leave documents as they are, not supported yet");
    assert_eq!(shown, shown.to_lowercase(), "labels are lowercase");
    // Other phrasings hold the same way, and next week's run holds again instead of hanging on a question.
    for text in ["clean up my documents folder", "every week tidy my documents"] {
        let (_, v) = run(&r.s, text);
        assert_eq!(v["state"]["held"], "folder-not-yet", "{text}: {v}");
        assert_eq!(count(&v, "action.requested"), 0);
    }
    assert_eq!(listing(&r.roots.downloads), downloads);
    assert_eq!(listing(&r.roots.desktop), desktop);
    // Downloads and the Desktop keep their own routes.
    let (_, v) = run(&r.s, "every week tidy my desktop");
    assert_eq!(v["state"]["status"], "done", "{v}");
}

/// The Screenshot app's preferences as macOS writes them (a binary plist) with `location`.
fn write_prefs(path: &Path, location: &str) {
    let mut dict = plist::Dictionary::new();
    dict.insert("location".into(), plist::Value::String(location.into()));
    dict.insert("type".into(), plist::Value::String("png".into()));
    plist::Value::Dictionary(dict).to_file_binary(path).unwrap();
}

// ADR 0126: the user saves screenshots to ~/Pictures/Screenshots. "tidy my desktop" moves the old captures there
// (no Desktop/screenshots folder is made), unattended; the preferences file is only read; put back returns them
// and leaves the user's own folder (ctrl didn't make it) in place.
#[test]
fn a_custom_screenshot_folder_is_where_the_desktop_captures_go_and_put_back_works() {
    let r = rig_with(true);
    typical(&r);
    let shots = r.d.path().join("Pictures/Screenshots");
    fs::create_dir_all(&shots).unwrap();
    put(&shots, "Screenshot 2026-10-17 at 1.00.00 PM.png", DAY);
    let prefs = r.roots.capture_prefs.clone().unwrap();
    write_prefs(&prefs, "~/Pictures/Screenshots");
    let prefs_before = (
        fs::read(&prefs).unwrap(),
        fs::metadata(&prefs).unwrap().modified().unwrap(),
    );
    assert_eq!(r.roots.screenshots_dir(), shots);
    let before = listing(&r.roots.desktop);
    let downloads = listing(&r.roots.downloads);

    let (t, v) = run(&r.s, "every week tidy my desktop");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(count(&v, "confirm.requested"), 0, "nothing deleted: no gate");
    assert_eq!(count(&v, "action.requested"), 1, "one batch");
    let mut want: Vec<String> = SHOTS.iter().map(|n| n.to_string()).collect();
    want.push("Screenshot 2026-10-17 at 1.00.00 PM.png".into());
    want.sort();
    assert_eq!(listing(&shots), want);
    let left: Vec<String> = after()
        .into_iter()
        .filter(|n| !n.starts_with("screenshots"))
        .collect();
    assert_eq!(listing(&r.roots.desktop), left, "no desktop/screenshots folder");
    assert_eq!(listing(&r.roots.downloads), downloads, "downloads untouched");
    assert!(listing(&r.roots.trash).is_empty(), "nothing trashed");
    let prefs_after = (
        fs::read(&prefs).unwrap(),
        fs::metadata(&prefs).unwrap().modified().unwrap(),
    );
    assert_eq!(prefs_before, prefs_after, "the preferences are only read");
    untouched(&r);
    no_names_on_disk(&r);
    for f in walk(&r.d.path().join("store")) {
        if f.is_file() {
            let text = fs::read_to_string(&f).unwrap_or_default().to_lowercase();
            assert!(
                !text.contains("pictures"),
                "the folder's path reached {}",
                f.display()
            );
        }
    }
    let log = Store::open(r.d.path().join("store"))
        .unwrap()
        .run(v["run"]["id"].as_str().unwrap())
        .unwrap();
    log.verify().unwrap();
    replay_check(
        &log,
        &Goal {
            task_id: &t,
            text: "every week tidy my desktop",
        },
    )
    .unwrap();

    let u = &v["state"]["undo"];
    assert_eq!(u["files"], 4, "{u}");
    assert_eq!(u["ready"], 4);
    let back =
        r.s.call("ctrl:undo", json!({"runId": v["run"]["id"], "rest": false}))
            .unwrap();
    assert_eq!(back["state"]["undo"]["done"]["undone"], 4, "{back}");
    assert_eq!(listing(&r.roots.desktop), before);
    assert_eq!(
        listing(&shots),
        vec!["Screenshot 2026-10-17 at 1.00.00 PM.png".to_string()],
        "the user's own folder and its capture stay"
    );
    untouched(&r);

    // The location changed back to the Desktop: the next run uses Desktop/screenshots again.
    write_prefs(&prefs, &r.roots.desktop.to_string_lossy());
    let (_, v) = run(&r.s, "move my screenshots off the desktop");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(listing(&r.roots.desktop), after());
}

// ADR 0126: only a real folder inside home that isn't the Desktop, Downloads, the Trash or ~/Library is used;
// anything else (missing, unreadable, not a plist, a symlink, `..`, outside home) falls back to Desktop/screenshots.
#[test]
fn the_capture_location_is_used_only_when_it_is_a_safe_folder() {
    let r = rig_with(true);
    let home = r.d.path();
    let prefs = r.roots.capture_prefs.clone().unwrap();
    let fallback = r.roots.desktop.join("screenshots");
    assert_eq!(capture_location(&prefs, &r.roots), None, "no preferences file");
    assert_eq!(r.roots.screenshots_dir(), fallback);
    for d in [
        "Pictures/Screenshots",
        "Desktop/caps",
        "Library/caps",
        "elsewhere",
    ] {
        fs::create_dir_all(home.join(d)).unwrap();
    }
    std::os::unix::fs::symlink(home.join("Pictures/Screenshots"), home.join("link")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let cases: Vec<(String, Option<PathBuf>)> = vec![
        (
            "~/Pictures/Screenshots".into(),
            Some(home.join("Pictures/Screenshots")),
        ),
        (
            home.join("Pictures/Screenshots/").to_string_lossy().into(),
            Some(home.join("Pictures/Screenshots")),
        ),
        ("~/Desktop/caps".into(), Some(home.join("Desktop/caps"))),
        ("~/Desktop".into(), None),
        ("~/Desktop/".into(), None),
        ("~/Downloads".into(), None),
        ("~/Trash".into(), None),
        ("~/Library/caps".into(), None),
        ("~".into(), None),
        ("~/missing".into(), None),
        ("~/link".into(), None),
        ("~/elsewhere/../Pictures/Screenshots".into(), None),
        ("Pictures/Screenshots".into(), None),
        (outside.path().to_string_lossy().into(), None),
        ("~someone/Pictures".into(), None),
        ("".into(), None),
    ];
    for (loc, want) in cases {
        write_prefs(&prefs, &loc);
        assert_eq!(capture_location(&prefs, &r.roots), want, "{loc:?}");
        assert_eq!(
            r.roots.screenshots_dir(),
            want.unwrap_or(fallback.clone()),
            "{loc:?}"
        );
    }
    // An XML plist reads the same; a file that isn't one, or a plist without a dictionary, is ignored.
    let mut dict = plist::Dictionary::new();
    dict.insert(
        "location".into(),
        plist::Value::String("~/Pictures/Screenshots".into()),
    );
    plist::Value::Dictionary(dict).to_file_xml(&prefs).unwrap();
    assert_eq!(
        capture_location(&prefs, &r.roots),
        Some(home.join("Pictures/Screenshots"))
    );
    fs::write(&prefs, b"location = ~/Pictures/Screenshots").unwrap();
    assert_eq!(capture_location(&prefs, &r.roots), None);
    plist::Value::String("~/Pictures/Screenshots".into())
        .to_file_binary(&prefs)
        .unwrap();
    assert_eq!(capture_location(&prefs, &r.roots), None);
    // A plan with a custom folder never makes Desktop/screenshots.
    write_prefs(&prefs, "~/Pictures/Screenshots");
    put(&r.roots.desktop, SHOTS[0], 2 * DAY);
    let p = plan_desktop_screenshots(&r.roots, false, at(T0)).unwrap();
    assert_eq!(p.files(), 1);
    assert!(!p.ops.iter().any(|o| matches!(o, Planned::MakeDir { .. })));
}

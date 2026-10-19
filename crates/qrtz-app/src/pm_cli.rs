//! ADR 0157 §9: `qrtz`, the package manager's command line, bun-style. It is a front end to the same
//! [`Packages`] the app uses (same installer, same fetcher, same verify path; Bozniak's rule: no separate
//! fetcher), and it runs no package code: an install only fetches, verifies, links and writes `qrtz.lock`.
//!
//! ```text
//! qrtz install [name[@range] …]   add requests and resolve against the signed index (no names: the lock as is)
//! qrtz install --frozen           exactly qrtz.lock, offline: relink from the store, fetch only missing blobs
//! qrtz install <file.qrtzpkg>     sideload a package file you picked (Bozniak's sideload rules)
//! qrtz remove <name> …            drop requests; what nothing else needs leaves the lock (no network)
//! qrtz update [name …] [--latest] re-resolve preferring the newest (`--latest` also moves the ranges)
//! qrtz engine <extension> <model> switch an extension's engine (a dependency change)
//! qrtz prune                      delete package dirs and blobs that qrtz.lock no longer references
//! qrtz ls                         installed packages
//! qrtz link [--dir <dir>]         opt in: symlink `qrtz` into a user-owned dir on PATH (never /usr/local)
//! qrtz unlink [--dir <dir>]
//!
//! -y, --yes      don't ask before changing anything      --offline  plan from the stored index only
//! --dry-run      show the plan and stop                  --root <dir>  data dir (default: the app's)
//! ```
use crate::module_hosting::{Applied, Change, InstallError, Op, Packages, Plan, RefreshConsent};
use qrtz_modules::sideload::{Confirmation, Refusal, SideloadOrigin};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub const USAGE: &str = "usage: qrtz <install|remove|update|engine|prune|ls|link|unlink> [args] [-y] [--offline] [--dry-run] [--root <dir>]";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub yes: bool,
    pub offline: bool,
    pub dry_run: bool,
    pub frozen: bool,
    pub latest: bool,
    pub root: Option<PathBuf>,
    pub dir: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Install(Vec<(String, Option<String>)>),
    Frozen,
    Sideload(PathBuf),
    Remove(Vec<String>),
    Update(Vec<String>),
    Engine(String, String),
    Prune,
    Ls,
    Link,
    Unlink,
}

/// `name` or `name@range` (names never contain `@`).
pub fn parse_spec(s: &str) -> (String, Option<String>) {
    match s.split_once('@') {
        Some((n, r)) => (n.to_owned(), Some(r.to_owned())),
        None => (s.to_owned(), None),
    }
}

pub fn parse(args: &[String]) -> Result<(Command, Flags), String> {
    let mut f = Flags::default();
    let mut pos = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-y" | "--yes" => f.yes = true,
            "--offline" => f.offline = true,
            "--dry-run" => f.dry_run = true,
            "--frozen" | "--frozen-lockfile" => f.frozen = true,
            "--latest" => f.latest = true,
            "--root" => f.root = Some(it.next().ok_or("--root needs a dir")?.into()),
            "--dir" => f.dir = Some(it.next().ok_or("--dir needs a dir")?.into()),
            x if x.starts_with('-') => return Err(format!("unknown flag {x}\n{USAGE}")),
            x => pos.push(x.to_owned()),
        }
    }
    let Some(verb) = pos.first().cloned() else {
        return Err(USAGE.into());
    };
    let rest = pos[1..].to_vec();
    let need = |what: &str| -> Result<(), String> {
        if rest.is_empty() {
            Err(format!("qrtz {verb}: name a {what}"))
        } else {
            Ok(())
        }
    };
    let cmd = match verb.as_str() {
        "install" | "i" | "add" => {
            if f.frozen {
                if !rest.is_empty() {
                    return Err("qrtz install --frozen takes no names".into());
                }
                Command::Frozen
            } else if rest.len() == 1 && rest[0].ends_with(".qrtzpkg") {
                Command::Sideload(rest[0].clone().into())
            } else {
                Command::Install(rest.iter().map(|s| parse_spec(s)).collect())
            }
        }
        "remove" | "rm" | "uninstall" => {
            need("package")?;
            Command::Remove(rest)
        }
        "update" | "up" | "upgrade" => Command::Update(rest),
        "engine" => match rest.as_slice() {
            [e, m] => Command::Engine(e.clone(), m.clone()),
            _ => return Err("qrtz engine <extension> <model>".into()),
        },
        "prune" => Command::Prune,
        "ls" | "list" => Command::Ls,
        "link" => Command::Link,
        "unlink" => Command::Unlink,
        v => return Err(format!("unknown command {v}\n{USAGE}")),
    };
    Ok((cmd, f))
}

fn mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / 1e6)
}

pub fn describe(plan: &Plan) -> String {
    let mut s = String::new();
    for c in &plan.changes {
        match c {
            Change::Add { name, version } => s += &format!("  + {name}@{version}\n"),
            Change::Remove { name, version } => s += &format!("  - {name}@{version}\n"),
        }
    }
    if plan.changes.is_empty() {
        s += "  (no package changes)\n";
    }
    s += &format!("  download {}", mb(plan.download_bytes));
    s
}

/// What a run talks to: stdout, and the yes/no answers (a tty, or a test's script).
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub ask: &'a mut dyn FnMut(&str) -> bool,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn e(x: impl std::fmt::Display) -> String {
    x.to_string()
}

/// Plan → show → ask → apply.
async fn run_op(
    p: &Packages,
    op: Op,
    f: &Flags,
    io: &mut Io<'_>,
    now: u64,
) -> Result<Option<Applied>, String> {
    let online = matches!(op, Op::Install(_) | Op::Update { .. } | Op::Engine { .. });
    if online && !f.offline {
        // The user typed the command: that is the consent to check the hosts.
        let r = p.refresh(&RefreshConsent::user_asked(), now).await.map_err(e)?;
        for n in &r.notices {
            writeln!(io.out, "notice: {n:?}").map_err(e)?;
        }
    }
    let plan = p.plan(op).map_err(e)?;
    writeln!(io.out, "{}", describe(&plan)).map_err(e)?;
    if plan.is_noop() || f.dry_run {
        return Ok(None);
    }
    if !f.yes && !(io.ask)("Proceed?") {
        writeln!(io.out, "nothing changed").map_err(e)?;
        return Ok(None);
    }
    let a = p
        .apply(&plan, &plan.consent(), now, &|_, _| {})
        .await
        .map_err(e)?;
    if a.stale_revocation {
        writeln!(
            io.out,
            "warning: installed with an expired revocation list; connect to refresh it"
        )
        .map_err(e)?;
    }
    writeln!(io.out, "done").map_err(e)?;
    Ok(Some(a))
}

pub async fn run(p: &Packages, cmd: Command, f: &Flags, io: &mut Io<'_>) -> Result<(), String> {
    let now = now_ms();
    match cmd {
        Command::Install(specs) if specs.is_empty() => {
            // Bare `qrtz install`: whatever qrtz.lock says, made whole (like `bun install`).
            run_op(p, Op::Frozen, f, io, now).await?;
        }
        Command::Install(specs) => {
            run_op(p, Op::Install(specs), f, io, now).await?;
        }
        Command::Frozen => {
            run_op(p, Op::Frozen, f, io, now).await?;
        }
        Command::Remove(names) => {
            run_op(p, Op::Remove(names), f, io, now).await?;
        }
        Command::Update(names) => {
            run_op(
                p,
                Op::Update {
                    names,
                    latest: f.latest,
                },
                f,
                io,
                now,
            )
            .await?;
        }
        Command::Engine(extension, model) => {
            run_op(p, Op::Engine { extension, model }, f, io, now).await?;
        }
        Command::Sideload(file) => sideload(p, &file, f, io, now).await?,
        Command::Prune => {
            let r = p.prune().map_err(e)?;
            writeln!(
                io.out,
                "pruned {} package dirs, {} blobs ({})",
                r.dirs.len(),
                r.blobs,
                mb(r.bytes)
            )
            .map_err(e)?;
        }
        Command::Ls => {
            for l in p.list().map_err(e)? {
                // The same users the module manager shows ([`qrtz_modules::qrtz_lock::QrtzLock::users`]).
                let why = match (&l.requested, l.used_by.is_empty()) {
                    (Some(r), true) => format!("requested {r}"),
                    (Some(r), false) => format!("requested {r}, for {}", l.used_by.join(", ")),
                    (None, false) => format!("for {}", l.used_by.join(", ")),
                    (None, true) => "unreferenced".into(),
                };
                let off = l.disabled.map(|d| format!("  DISABLED: {d}")).unwrap_or_default();
                writeln!(
                    io.out,
                    "{}@{}  {:?}  {}  {}{off}",
                    l.name,
                    l.version,
                    l.kind,
                    mb(l.size),
                    why
                )
                .map_err(e)?;
            }
        }
        Command::Link => {
            let dir = f.dir.clone().or_else(default_link_dir).ok_or("no home dir")?;
            let target = std::env::current_exe().map_err(e)?;
            let p = link(&dir, &target)?;
            writeln!(io.out, "linked {} -> {}", p.display(), target.display()).map_err(e)?;
        }
        Command::Unlink => {
            let dir = f.dir.clone().or_else(default_link_dir).ok_or("no home dir")?;
            unlink(&dir)?;
            writeln!(io.out, "unlinked {}", dir.join("qrtz").display()).map_err(e)?;
        }
    }
    Ok(())
}

async fn sideload(p: &Packages, file: &Path, f: &Flags, io: &mut Io<'_>, now: u64) -> Result<(), String> {
    // The user named this file on the command line: a user-picked file (rule 4).
    let origin = SideloadOrigin::UserPickedFile;
    let prompt = match p.installer.sideload(file, origin, None, now).await {
        Err(InstallError::Refused(Refusal::NeedsConfirmation(pr))) => pr,
        Err(x) => return Err(x.to_string()),
        Ok(_) => return Err("installed without a confirmation".into()),
    };
    // ADR 0174: a file can't shadow an extension qrtz.app bundles (echo).
    if qrtz_modules::is_bundled_extension(&prompt.module) {
        return Err(format!(
            "{} is built into qrtz.app: a file can't replace it",
            prompt.module
        ));
    }
    let age = match (prompt.revocation_expired, prompt.revocation_age_ms) {
        (true, Some(a)) => format!(" (its revocation list is {} days old)", a / 86_400_000),
        (true, None) => " (no revocation list)".into(),
        _ => String::new(),
    };
    writeln!(
        io.out,
        "install {}@{} from {}{age}",
        prompt.module,
        prompt.version,
        file.display()
    )
    .map_err(e)?;
    if f.dry_run || (!f.yes && !(io.ask)("Install this file?")) {
        writeln!(io.out, "nothing changed").map_err(e)?;
        return Ok(());
    }
    let yes = Confirmation {
        package_sha256: prompt.package_sha256,
        revocation_issued_ms: prompt.revocation_issued_ms,
    };
    let a = p
        .installer
        .sideload(file, origin, Some(&yes), now)
        .await
        .map_err(e)?;
    writeln!(io.out, "installed {} packages", a.changes.len()).map_err(e)?;
    Ok(())
}

/// `~/.local/bin`.
pub fn default_link_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin"))
}

/// Bozniak's PATH rule: an opt-in symlink, in a dir the user owns, to the binary inside the app bundle. Never a
/// root-owned dir (no `/usr/local`, no sudo), never a copy.
pub fn link(dir: &Path, target: &Path) -> Result<PathBuf, String> {
    if dir.starts_with("/usr")
        || dir.starts_with("/bin")
        || dir.starts_with("/sbin")
        || dir.starts_with("/opt")
    {
        return Err(format!(
            "{}: qrtz links only into a dir you own (e.g. ~/.local/bin)",
            dir.display()
        ));
    }
    #[cfg(target_os = "macos")]
    if !target.to_string_lossy().contains(".app/Contents/MacOS/") {
        return Err("run `qrtz link` from the qrtz inside qrtz.app".into());
    }
    std::fs::create_dir_all(dir).map_err(e)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(dir).map_err(e)?;
        // Owned by the user: the owner of $HOME, and never root.
        let home_uid = std::env::var_os("HOME")
            .and_then(|h| std::fs::metadata(h).ok())
            .map(|m| m.uid());
        if m.uid() == 0 || home_uid.is_some_and(|u| u != m.uid()) {
            return Err(format!("{}: not owned by you", dir.display()));
        }
    }
    let p = dir.join("qrtz");
    match std::fs::symlink_metadata(&p) {
        Ok(m) if m.file_type().is_symlink() => std::fs::remove_file(&p).map_err(e)?,
        Ok(_) => return Err(format!("{}: exists and isn't our link", p.display())),
        Err(_) => {}
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &p).map_err(e)?;
    #[cfg(not(unix))]
    return Err("qrtz link needs a unix".into());
    #[cfg(unix)]
    Ok(p)
}

pub fn unlink(dir: &Path) -> Result<(), String> {
    let p = dir.join("qrtz");
    match std::fs::symlink_metadata(&p) {
        Ok(m) if m.file_type().is_symlink() => std::fs::remove_file(&p).map_err(e),
        Ok(_) => Err(format!("{}: not a link; left alone", p.display())),
        Err(_) => Ok(()),
    }
}

/// A y/N question on the terminal.
pub fn ask_tty(q: &str) -> bool {
    print!("{q} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    std::io::stdin().lock().read_line(&mut s).is_ok() && matches!(s.trim(), "y" | "Y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn bun_style_verbs_parse() {
        assert_eq!(
            parse(&a("install echo nano@^0.3")).unwrap().0,
            Command::Install(vec![("echo".into(), None), ("nano".into(), Some("^0.3".into()))])
        );
        assert_eq!(parse(&a("i --frozen")).unwrap().0, Command::Frozen);
        assert_eq!(
            parse(&a("install ./echo.qrtzpkg")).unwrap().0,
            Command::Sideload("./echo.qrtzpkg".into())
        );
        assert_eq!(
            parse(&a("rm echo")).unwrap().0,
            Command::Remove(vec!["echo".into()])
        );
        let (c, f) = parse(&a("update --latest -y")).unwrap();
        assert_eq!((c, f.latest, f.yes), (Command::Update(vec![]), true, true));
        assert_eq!(
            parse(&a("engine echo model.nemotron")).unwrap().0,
            Command::Engine("echo".into(), "model.nemotron".into())
        );
        assert!(parse(&a("remove")).is_err());
        assert!(parse(&a("install --frozen echo")).is_err());
        assert!(parse(&a("install --postinstall")).is_err());
    }

    #[test]
    fn link_refuses_system_dirs_and_replaces_only_its_own_link() {
        assert!(link(Path::new("/usr/local/bin"), Path::new("/x")).is_err());
        assert!(link(Path::new("/opt/homebrew/bin"), Path::new("/x")).is_err());
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let d = tempfile::tempdir().unwrap();
            let p = link(d.path(), Path::new("/x/qrtz")).unwrap();
            assert_eq!(std::fs::read_link(&p).unwrap(), Path::new("/x/qrtz"));
            link(d.path(), Path::new("/y/qrtz")).unwrap();
            assert_eq!(std::fs::read_link(&p).unwrap(), Path::new("/y/qrtz"));
            unlink(d.path()).unwrap();
            std::fs::write(&p, b"someone's file").unwrap();
            assert!(link(d.path(), Path::new("/x/qrtz")).is_err());
            assert!(unlink(d.path()).is_err());
            assert_eq!(std::fs::read(&p).unwrap(), b"someone's file");
        }
    }
}

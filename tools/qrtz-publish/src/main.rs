//! `qrtz-publish`: see `docs/modules/hosting-setup.md` and ADR 0153.
use qrtz_publish::keys::{write_keypair, Keys};
use std::path::PathBuf;

const USAGE: &str = "qrtz-publish <command> [options]

  keygen    --dir <keys> --role <root-a|root-b|timestamp|index>   new key (0600) + .pub; never overwrites
  root      --out <dir> [--keys <dir>] [--prev-keys <dir>] [--threshold 1]
                                      write root N+1 (or 1) and print its sha256 (pin 1.root.json's in the app)
  build     --out <dir> --spec <publish.json> [--keys <dir>]       add packages, sign index + timestamp
  timestamp --out <dir> [--keys <dir>]                              re-sign the timestamp (at least weekly)
  pack      --out <dir> --module <id> --version <v> --to <file.qrtzpkg>
  verify    --out <dir> --root-sha256 <hex>                         check the dir as a fresh client would
  dry-run   [--dir <dir>] [--big]     everything with test keys made at runtime (never written)

keys: --keys <dir>, or the env QRTZ_PUBLISH_ROOT_KEYS / QRTZ_PUBLISH_TIMESTAMP_KEY / QRTZ_PUBLISH_INDEX_KEY.
";

fn main() {
    if let Err(e) = run() {
        eprintln!("qrtz-publish: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        print!("{USAGE}");
        return Ok(());
    };
    let opt = |k: &str| -> Option<String> {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let need = |k: &str| opt(k).ok_or(format!("{cmd}: {k} is required\n\n{USAGE}"));
    let flag = |k: &str| args.iter().any(|a| a == k);
    let keys = || -> Result<Keys, String> {
        match opt("--keys") {
            Some(d) => Keys::from_dir(&PathBuf::from(d)),
            None => Keys::from_env(),
        }
    };
    let now = qrtz_publish::now_ms();
    match cmd.as_str() {
        "keygen" => {
            let dir = PathBuf::from(need("--dir")?);
            let role = need("--role")?;
            let ok = role == "timestamp" || role == "index" || (role.starts_with("root-") && role.len() > 5);
            if !ok {
                return Err("--role: root-<name>, timestamp or index".into());
            }
            let k = write_keypair(&dir, &role)?;
            println!(
                "{role}: wrote {role}.key (0600) and {role}.pub; key id {}",
                qrtz_modules::index::key_id(&k.to_bytes())
            );
        }
        "root" => {
            let prev = opt("--prev-keys")
                .map(|d| Keys::from_dir(&PathBuf::from(d)))
                .transpose()?;
            let t: u32 =
                opt("--threshold").map_or(Ok(1), |t| t.parse().map_err(|_| "--threshold: a number"))?;
            let (n, sha) =
                qrtz_publish::write_root(&PathBuf::from(need("--out")?), &keys()?, prev.as_ref(), t, now)?;
            println!("root {n}: sha256 {sha}");
        }
        "build" => {
            let spec = qrtz_publish::Spec::load(&PathBuf::from(need("--spec")?))?;
            let b = qrtz_publish::build(&PathBuf::from(need("--out")?), &spec, &keys()?, now)?;
            println!(
                "index {} ({}) and timestamp {}; added: {}",
                b.index_version,
                b.index_sha256,
                b.timestamp_version,
                if b.added.is_empty() {
                    "nothing new".into()
                } else {
                    b.added.join(", ")
                }
            );
        }
        "timestamp" => {
            let t = qrtz_publish::resign_timestamp(&PathBuf::from(need("--out")?), &keys()?, now)?;
            println!("timestamp {} for index {}", t.version, t.index.version);
        }
        "pack" => {
            let to = PathBuf::from(need("--to")?);
            qrtz_publish::pack(
                &PathBuf::from(need("--out")?),
                &need("--module")?,
                &need("--version")?,
                &to,
            )?;
            println!("wrote {}", to.display());
        }
        "verify" => println!(
            "{}",
            qrtz_publish::verify(&PathBuf::from(need("--out")?), &need("--root-sha256")?, now)?
        ),
        "dry-run" => {
            let tmp;
            let dir = match opt("--dir") {
                Some(d) => PathBuf::from(d),
                None => {
                    tmp = std::env::temp_dir().join(format!("qrtz-publish-dry-run-{}", std::process::id()));
                    tmp.clone()
                }
            };
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            for l in qrtz_publish::dry_run::run(&dir, flag("--big"))? {
                println!("{l}");
            }
            println!("output in {}", dir.display());
        }
        "-h" | "--help" | "help" => print!("{USAGE}"),
        _ => return Err(format!("unknown command {cmd}\n\n{USAGE}")),
    }
    Ok(())
}

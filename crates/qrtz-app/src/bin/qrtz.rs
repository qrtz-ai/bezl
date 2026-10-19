//! `qrtz`: the package manager's command line (ADR 0157 §9). See `qrtz_app::pm_cli`. Shipped as
//! `qrtz.app/Contents/MacOS/qrtz` (a Tauri externalBin; the app itself is `Contents/MacOS/qrtz-app`, ADR 0160).
use qrtz_app::module_hosting::Packages;
use qrtz_app::pm_cli::{ask_tty, parse, run, Io};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("help" | "-h" | "--help")) {
        println!("{}", qrtz_app::pm_cli::USAGE);
        return;
    }
    let (cmd, flags) = match parse(&args) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let Some(p) = Packages::pinned(flags.root.clone()) else {
        eprintln!("qrtz: no data dir (set HOME or QRTZ_USER_DATA, or pass --root)");
        std::process::exit(2);
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let mut out = std::io::stdout();
    let mut ask = |q: &str| ask_tty(q);
    let mut io = Io {
        out: &mut out,
        ask: &mut ask,
    };
    if let Err(e) = rt.block_on(run(&p, cmd, &flags, &mut io)) {
        eprintln!("qrtz: {e}");
        std::process::exit(1);
    }
}

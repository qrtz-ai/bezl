//! ADR 0078: "qrtz is entirely offline" (ADR 0076) as a property of the build, not a list of removed features.
//!
//! 1. Dependency graph (`cargo metadata`, normal edges only, for the macOS and Linux targets): from the app
//!    binary (`qrtz-desktop`) no HTTP, WebSocket, TLS or network-plugin crate is reachable except through the one
//!    documented exception, `qrtz-speech → reqwest` (the signed speech module update channel, ADR 0046 §4).
//!    Third-party crates (tauri and its plugins) pull none on these targets, and no other workspace crate names
//!    one directly.
//! 2. Sources: only `qrtz-speech/src/download.rs` names `reqwest`; no Rust source opens a TCP/UDP socket outside
//!    the speech test kit.
//! 3. Tauri: the CSP keeps fetch/WebSocket to the app itself and IPC, and the window capability grants no
//!    http/opener/shell/updater/websocket permission.
//! 4. Helper: no Private Cloud Compute model and no network entitlement.
//! 5. ADR 0157 (tightening ADR 0079 and 0153): the downloader's only callers are qrtz-app's `setup.rs` (the signed
//!    speech bundle, `Downloader` only) and `module_hosting.rs` (the package manager's one fetcher, `fetch_meta` and
//!    `fetch_range`, to the host pinned in `module_hosts.json`: exactly `https://modules.qrtz.ai` (ADR 0167: no
//!    Backblaze B2 mirror), and `modelMirrors`, exactly `https://huggingface.co`). `local_models.rs` keeps only the pause/progress types
//!    (`DownloadControl`, `Progress`); naming `Downloader` or any fetch there (the retired direct-HF
//!    `download_files` path) fails, and `download_files` has no caller outside qrtz-speech's `download_bundle`.
//!    The pinned models are fixed Hugging Face revisions with sha256s; ctrl's Laya model (ADR 0116) goes through
//!    `local_models.rs` too (`ctrl_model.rs` names no downloader).
//!    ADR 0157 §9: at most one `extension.reads` exception in any package manifest (ctrl reading `model.laya`).
//!
//! 6. ADR 0144 (module confinement, M0): every crate and Swift target has an owner in `docs/modules/confinement-map.json`
//!    (shell / shared / echo / nano / ctrl / voice / monolith); module-owned and shared crates reach no network crate in
//!    their own build and name no downloader; cross-owner edges (the monolith) are an exact allowlist that may only
//!    shrink; module and inference XPC entitlements files are linted by name.
//!    ADR 0154 (one qrtz identity): extension helper entitlements (`ai.qrtz.<echo|nano|ctrl>`) are accepted, not
//!    required, and may hold only the app's TCC allowlist (`appTccEntitlements`, each entry with an ADR note); inference
//!    XPC services stay strict (app-sandbox, no network/device/personal-information/automation).
//!
//! 7. ADR 0231 amendment 1: every check above applies to every crate except one mapped to a network role in
//!    `docs/modules/confinement-map.json` (`networkRoles`; today only qrtz-dash-net). A network-role crate is held
//!    to its own rule instead (`network_role_crates_*`): no workspace crate links it, its direct network crates are
//!    exactly its `netCrates`, its workspace dependencies are shared-owned only, and its sources name no URL outside
//!    its `egress`. qrtz-app's own graph still reaches no network crate but the speech channel.
//!
//! A new network path has to change this test and its ADR, on purpose.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};

/// Crates that give a process an HTTP/WebSocket/TLS client or server, or hand one to the renderer.
const NET_CRATES: &[&str] = &[
    "reqwest",
    "hyper",
    "hyper-util",
    "hyper-rustls",
    "hyper-tls",
    "h2",
    "h3",
    "quinn",
    "tungstenite",
    "tokio-tungstenite",
    "async-tungstenite",
    "websocket",
    "ureq",
    "isahc",
    "curl",
    "curl-sys",
    "surf",
    "attohttpc",
    "native-tls",
    "tokio-native-tls",
    "rustls",
    "tokio-rustls",
    "openssl",
    "openssl-sys",
    "hickory-resolver",
    "trust-dns-resolver",
    "axum",
    "actix-web",
    "warp",
    "tiny_http",
    "tauri-plugin-http",
    "tauri-plugin-updater",
    "tauri-plugin-websocket",
    "tauri-plugin-upload",
    "tauri-plugin-opener",
    "tauri-plugin-shell",
];

/// The one allowed edge into NET_CRATES (ADR 0046 §4, ADR 0078), behind qrtz-speech's `net` feature.
const EXCEPTION: (&str, &str) = ("qrtz-speech", "reqwest");
/// The feature that carries the exception, and the one crate allowed to enable it (ADR 0078 follow-up).
const NET_FEATURE: &str = "net";
const NET_FEATURE_USERS: &[&str] = &["qrtz-app"];
/// Crates that use qrtz-speech without its network channel: built alone, they reach no network crate.
const SPEECH_WITHOUT_NET: &[&str] = &["qrtz-speech", "qrtz-talk", "qrtz-lifecycle"];

/// Core crates that must not reach any network crate even through qrtz-speech (they don't depend on it).
const NET_FREE: &[&str] = &[
    "qrtz-model",
    "qrtz-storage",
    "qrtz-llm",
    "qrtz-backup",
    "qrtz-ipc",
    "qrtz-cloexec",
];

const TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Graph {
    name: HashMap<String, String>,
    normal: HashMap<String, Vec<String>>,
    members: BTreeSet<String>,
}

fn graph(target: &str) -> Graph {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = std::process::Command::new(cargo)
        .current_dir(root())
        .args(["metadata", "--format-version", "1", "--filter-platform", target])
        .output()
        .expect("cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let m: Value = serde_json::from_slice(&out.stdout).unwrap();
    let name = m["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["id"].as_str().unwrap().to_owned(),
                p["name"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<HashMap<_, _>>();
    let members = m["workspace_members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let mut normal = HashMap::new();
    for n in m["resolve"]["nodes"].as_array().unwrap() {
        let deps = n["deps"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| {
                d["dep_kinds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|k| k["kind"].is_null())
            })
            .map(|d| d["pkg"].as_str().unwrap().to_owned())
            .collect();
        normal.insert(n["id"].as_str().unwrap().to_owned(), deps);
    }
    Graph {
        name,
        normal,
        members,
    }
}

impl Graph {
    fn id(&self, name: &str) -> String {
        self.members
            .iter()
            .find(|id| self.name[*id] == name)
            .unwrap_or_else(|| panic!("{name} is not a workspace member"))
            .clone()
    }
    /// Network crates reachable from `from` over normal edges, with the path to each; `cut` drops the exception.
    fn reach(&self, from: &str, cut: bool) -> Vec<String> {
        let start = self.id(from);
        let mut prev: HashMap<String, String> = HashMap::new();
        let mut seen = BTreeSet::from([start.clone()]);
        let mut q = VecDeque::from([start]);
        let mut hits = vec![];
        while let Some(at) = q.pop_front() {
            for d in &self.normal[&at] {
                if cut && self.name[&at] == EXCEPTION.0 && self.name[d] == EXCEPTION.1 {
                    continue;
                }
                if seen.insert(d.clone()) {
                    prev.insert(d.clone(), at.clone());
                    if NET_CRATES.contains(&self.name[d].as_str()) {
                        let mut p = vec![self.name[d].clone()];
                        let mut c = d.clone();
                        while let Some(b) = prev.get(&c) {
                            p.push(self.name[b].clone());
                            c = b.clone();
                        }
                        p.reverse();
                        hits.push(p.join(" → "));
                    }
                    q.push_back(d.clone());
                }
            }
        }
        hits
    }
}

#[test]
fn the_app_reaches_no_network_crate_except_the_speech_update_channel() {
    for t in TARGETS {
        let g = graph(t);
        let cut = g.reach("qrtz-desktop", true);
        assert!(
            cut.is_empty(),
            "{t}: network crates outside the ADR 0078 exception:\n{}",
            cut.join("\n")
        );
        // The exception is real (so this test notices when it goes away and can be tightened).
        assert!(
            g.reach("qrtz-desktop", false)
                .iter()
                .any(|p| p.ends_with("qrtz-speech → reqwest")),
            "{t}: qrtz-speech no longer pulls reqwest; drop the exception in this test and ADR 0078"
        );
        for c in NET_FREE {
            let r = g.reach(c, false);
            assert!(r.is_empty(), "{t}: {c} reaches the network:\n{}", r.join("\n"));
        }
    }
}

/// ADR 0231 amendment 1: crates mapped to a network role (`networkRoles.crates` in the confinement map). The
/// offline checks skip exactly these; `network_role_crates_*` hold them to their own rule.
fn network_roles() -> BTreeMap<String, Value> {
    let p = root().join("docs/modules/confinement-map.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    v["networkRoles"]["crates"]
        .as_object()
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

#[test]
fn only_qrtz_speech_names_a_network_crate_directly() {
    let g = graph("aarch64-apple-darwin");
    let roles = network_roles();
    let mut direct = vec![];
    for m in &g.members {
        if roles.contains_key(&g.name[m]) {
            continue;
        }
        for d in &g.normal[m] {
            if NET_CRATES.contains(&g.name[d].as_str()) {
                direct.push((g.name[m].clone(), g.name[d].clone()));
            }
        }
    }
    assert_eq!(direct, vec![(EXCEPTION.0.to_owned(), EXCEPTION.1.to_owned())]);
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            let n = p.file_name().unwrap().to_string_lossy();
            if n != "tests" && n != "target" && n != "gen" {
                rust_sources(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Code lines (not `//` comments) up to the first `#[cfg(test)]` (unit tests may bind loopback listeners).
fn code(p: &Path) -> Vec<(usize, String)> {
    let s = std::fs::read_to_string(p).unwrap();
    s.lines()
        .enumerate()
        .take_while(|(_, l)| !l.trim_start().starts_with("#[cfg(test)]"))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .map(|(i, l)| (i + 1, l.to_owned()))
        .collect()
}

#[test]
fn no_rust_source_opens_a_socket_or_names_an_http_client_outside_the_exception() {
    let r = root();
    let mut files = vec![];
    rust_sources(&r.join("crates"), &mut files);
    rust_sources(&r.join("apps/desktop/src-tauri/src"), &mut files);
    // ADR 0153: release tools (qrtz-publish) are held to the same rule: they write files, never sockets.
    rust_sources(&r.join("tools"), &mut files);
    assert!(files.len() > 50, "found only {} sources", files.len());
    let roles: Vec<String> = network_roles().keys().map(|c| format!("crates/{c}/")).collect();
    let mut bad = vec![];
    for f in &files {
        let rel = f.strip_prefix(&r).unwrap().to_string_lossy().replace('\\', "/");
        if roles.iter().any(|p| rel.starts_with(p.as_str())) {
            continue;
        }
        // The speech test kit (feature `testkit`, never in the app) serves bundles on 127.0.0.1.
        if rel.ends_with("qrtz-speech/src/testkit.rs")
            || rel.contains("/examples/")
            || rel.contains("/bin/qrtz-mock-")
        {
            continue;
        }
        for (n, l) in code(f) {
            let client = l.contains("reqwest") || l.contains("tungstenite") || l.contains("ureq::");
            let socket = [
                "TcpStream",
                "TcpListener",
                "UdpSocket",
                "TcpSocket",
                "ToSocketAddrs",
            ]
            .iter()
            .any(|k| l.contains(k));
            if socket || (client && !rel.ends_with("qrtz-speech/src/download.rs")) {
                bad.push(format!("{rel}:{n}: {}", l.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "network use outside ADR 0078's exception:\n{}",
        bad.join("\n")
    );
}

/// ADR 0157 §5/§6: what reaches the network through qrtz-speech's downloader. Naming any of these is a fetch (or
/// a way to make one): the transport type, its constructor's type, and its fetching methods.
const FETCH_NAMES: &[&str] = &[
    "Downloader",
    "fetch_meta",
    "fetch_range",
    "download_bundle",
    "download_files",
];
/// The downloader's pause and progress types: plain data, no transport.
const DOWNLOAD_TYPES: &[&str] = &["DownloadControl", "Progress"];
/// The only sources that may name a fetch, and which names each may use:
/// - `setup.rs`: the signed speech bundle (ADR 0046), a `Downloader` handed to qrtz-speech's `Updater`
///   (whose `download_bundle` is the only caller of `download_files`).
/// - `module_hosting.rs`: the package manager's one fetcher (ADR 0157 §5), `HttpTransport` over `fetch_meta` and
///   `fetch_range`, to the pinned hosts and `modelMirrors` only (checked below).
const FETCH_CALLERS: &[(&str, &[&str])] = &[
    ("crates/qrtz-app/src/setup.rs", &["Downloader"]),
    (
        "crates/qrtz-app/src/module_hosting.rs",
        &["Downloader", "fetch_meta", "fetch_range"],
    ),
];
/// Sources that may import the pause/progress types from `qrtz_speech::download`, and nothing else from it.
/// `local_models.rs` (ADR 0079) no longer fetches: it plans and applies through the package manager and keeps only
/// `DownloadControl` and `Progress` (ADR 0157 §6). Its direct Hugging Face path (`download_files` with the model
/// host as base) is retired; bringing it back fails here.
const TYPE_IMPORTERS: &[&str] = &["crates/qrtz-app/src/local_models.rs"];

fn idents(l: &str) -> impl Iterator<Item = &str> {
    l.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
}

/// Every use of qrtz-speech's downloader in one source (`rel`, code lines as [`code`] returns them) that ADR 0157
/// doesn't allow there.
fn downloader_uses(rel: &str, lines: &[(usize, String)]) -> Vec<String> {
    let callers = FETCH_CALLERS
        .iter()
        .find(|(f, _)| *f == rel)
        .map(|(_, names)| *names);
    let mut bad = vec![];
    for (n, l) in lines {
        let flat: String = l.chars().filter(|c| !c.is_whitespace()).collect();
        let mut why = None;
        if let Some(w) =
            idents(l).find(|w| FETCH_NAMES.contains(w) && !callers.is_some_and(|c| c.contains(w)))
        {
            why = Some(format!("names `{w}`"));
        }
        if why.is_none() && callers.is_none() {
            // `speech::download` (any import or path into the module): only the pause/progress types, only in a
            // TYPE_IMPORTERS source, as `…::download::{DownloadControl, Progress}` or `…::download::Progress`.
            let mut rest = flat.as_str();
            while let Some(i) = rest.find("speech::download") {
                rest = &rest[i + "speech::download".len()..];
                let ok = TYPE_IMPORTERS.contains(&rel)
                    && rest.strip_prefix("::").is_some_and(|t| {
                        let names: Vec<&str> = match t.strip_prefix('{') {
                            Some(list) => match list.split_once('}') {
                                Some((inner, _)) => inner.split(',').filter(|x| !x.is_empty()).collect(),
                                // A `{` list over several lines: refuse rather than guess what follows.
                                None => vec!["…"],
                            },
                            None => vec![idents(t).next().unwrap_or("")],
                        };
                        !names.is_empty() && names.iter().all(|x| DOWNLOAD_TYPES.contains(x))
                    });
                if !ok {
                    why = Some("imports from qrtz_speech::download beyond its pause/progress types".into());
                    break;
                }
            }
        }
        if let Some(w) = why {
            bad.push(format!("{rel}:{n}: {w}: {}", l.trim()));
        }
    }
    bad
}

#[test]
fn only_setup_and_the_package_fetcher_call_the_downloader() {
    let r = root();
    let mut files = vec![];
    rust_sources(&r.join("crates"), &mut files);
    rust_sources(&r.join("apps/desktop/src-tauri/src"), &mut files);
    rust_sources(&r.join("tools"), &mut files);
    let mut bad = vec![];
    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in &files {
        let rel = f.strip_prefix(&r).unwrap().to_string_lossy().replace('\\', "/");
        if rel.contains("/tests/") || rel.contains("/examples/") {
            continue;
        }
        let lines = code(f);
        if let Some(inner) = rel.strip_prefix("crates/qrtz-speech/src/") {
            // Inside qrtz-speech: `download_files` (the generic pinned-list transfer that served the retired direct-HF
            // model path) is called only by `download_bundle`, in download.rs itself.
            if inner != "download.rs" && inner != "testkit.rs" {
                for (n, l) in &lines {
                    if idents(l).any(|w| w == "download_files") {
                        bad.push(format!(
                            "{rel}:{n}: names `download_files` outside download.rs: {}",
                            l.trim()
                        ));
                    }
                }
            }
            continue;
        }
        for (_, l) in &lines {
            for w in idents(l).filter(|w| FETCH_NAMES.contains(w) || DOWNLOAD_TYPES.contains(w)) {
                seen.entry(rel.clone()).or_default().insert(w.to_owned());
            }
        }
        bad.extend(downloader_uses(&rel, &lines));
    }
    assert!(
        bad.is_empty(),
        "downloader used outside ADR 0157's callers (setup.rs: the speech bundle; module_hosting.rs: the one \
         package fetcher; local_models.rs: the pause/progress types only):\n{}",
        bad.join("\n")
    );
    // The allowlist is real, so this test notices when an entry goes away and can be tightened.
    for (f, names) in FETCH_CALLERS {
        for w in *names {
            assert!(
                seen.get(*f).is_some_and(|s| s.contains(*w)),
                "{f} no longer names `{w}`: drop it from FETCH_CALLERS"
            );
        }
    }
    for f in TYPE_IMPORTERS {
        assert!(
            seen.get(*f)
                .is_some_and(|s| DOWNLOAD_TYPES.iter().any(|t| s.contains(*t))),
            "{f} no longer uses the downloader's pause/progress types: drop it from TYPE_IMPORTERS"
        );
    }
    let fetchers: Vec<&String> = seen
        .iter()
        .filter(|(_, s)| s.contains("fetch_range"))
        .map(|(f, _)| f)
        .collect();
    assert_eq!(
        fetchers,
        ["crates/qrtz-app/src/module_hosting.rs"],
        "ADR 0157 §5: one fetcher for every package kind"
    );

    for manifest in ["local_models.json", "ctrl_laya_model.json"] {
        let m: Value = serde_json::from_str(
            &std::fs::read_to_string(r.join("crates/qrtz-app/src").join(manifest)).unwrap(),
        )
        .unwrap();
        assert_eq!(m["base"], "https://huggingface.co", "{manifest}");
        for set in m["sets"].as_array().unwrap() {
            let rev = set["revision"].as_str().unwrap();
            assert!(
                rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()),
                "{manifest} {set}: a fixed commit, not a branch"
            );
            for f in set["files"].as_array().unwrap() {
                assert_eq!(f["sha256"].as_str().unwrap().len(), 64, "{manifest} {f}");
            }
        }
    }
    // ADR 0153 / ADR 0167: exactly one package host (Cloudflare R2 behind modules.qrtz.ai), no B2 mirror;
    // https, no path, query or credentials. Adding a host changes this test and the ADR.
    let hosts: Value = serde_json::from_str(
        &std::fs::read_to_string(r.join("crates/qrtz-app/src/module_hosts.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        hosts["primary"], "https://modules.qrtz.ai",
        "module_hosts.json primary"
    );
    let mirrors = hosts["mirrors"].as_array().unwrap();
    assert!(
        mirrors.is_empty(),
        "module_hosts.json: no package-host mirror (ADR 0167), got {mirrors:?}"
    );
    // ADR 0157: the only extra mirror host a signed tree may point at (the pinned models' upstream).
    assert_eq!(
        hosts["modelMirrors"],
        serde_json::json!(["https://huggingface.co"]),
        "module_hosts.json modelMirrors"
    );
    // Root 1 is pinned (2026-10-18 key ceremony). Its public copy is checked in next to the verifier, and
    // crates/qrtz-modules/tests/pinned_root.rs checks that it hashes to this pin and meets threshold 2.
    let pin = &hosts["root1Sha256"];
    assert!(
        pin.as_str()
            .is_some_and(|p| p.len() == 64 && p.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))),
        "root1Sha256: a lowercase sha256, got {pin}"
    );
    for f in ["1.root.json", "1.root.json.sig"] {
        assert!(
            r.join("crates/qrtz-modules/roots").join(f).is_file(),
            "the pinned root's public {f} is checked in at crates/qrtz-modules/roots/"
        );
    }
    // ADR 0116: ctrl's model is fetched through the pinned path, not a downloader of its own.
    let ctrl = std::fs::read_to_string(r.join("crates/qrtz-app/src/ctrl_model.rs")).unwrap();
    assert!(
        ctrl.contains("LocalModels"),
        "ctrl_model.rs downloads through local_models"
    );
}

#[test]
fn the_downloader_ratchet_refuses_a_fetch_from_local_models() {
    let lm = "crates/qrtz-app/src/local_models.rs";
    let lines = |src: &[&str]| -> Vec<(usize, String)> {
        src.iter()
            .enumerate()
            .map(|(i, l)| (i + 1, (*l).to_owned()))
            .collect()
    };
    // Today's shape: the pause/progress types, re-exported for ctrl_model.rs, and the package fetcher.
    assert_eq!(
        downloader_uses(
            lm,
            &lines(&[
                "pub use qrtz_speech::download::{DownloadControl, Progress};",
                "use qrtz_speech::download::Progress;",
                "use crate::module_hosting::{AppleCodeChecks, Hosts, HttpTransport, InstallError, Op};",
                "let t = HttpTransport::pinned();",
            ])
        ),
        Vec::<String>::new()
    );
    // The retired direct path, in each shape it could come back.
    for bad in [
        "use qrtz_speech::download::{DownloadControl, Downloader, Progress};",
        "use qrtz_speech::download::*;",
        "use qrtz_speech::download;",
        "use qrtz_speech::download as dl;",
        "use qrtz_speech::download::{",
        "let dl = Downloader::new(&base);",
        "let dl = qrtz_speech::download::Downloader::new(&base);",
        "dl.download_files(&files, &staging, &ctl, &on_progress).await?;",
        "dl.fetch_range(&path, 0, 8, &ctl).await?;",
        "dl.fetch_meta(\"x\").await?;",
        "struct Fetch { dl: Downloader }",
    ] {
        assert_eq!(downloader_uses(lm, &lines(&[bad])).len(), 1, "{bad}");
    }
    // Any other source may not even import the types from qrtz-speech (it takes them from local_models).
    assert_eq!(
        downloader_uses(
            "crates/qrtz-app/src/ctrl_model.rs",
            &lines(&["use qrtz_speech::download::Progress;"])
        )
        .len(),
        1
    );
    // The two callers keep only their own names: setup.rs can't take the range fetcher.
    assert_eq!(
        downloader_uses(
            "crates/qrtz-app/src/setup.rs",
            &lines(&["dl.fetch_range(&p, 0, 8, &ctl).await?;"])
        )
        .len(),
        1
    );
}

#[test]
fn the_renderer_can_only_reach_the_app_and_ipc() {
    let r = root().join("apps/desktop/src-tauri");
    let conf: Value =
        serde_json::from_str(&std::fs::read_to_string(r.join("tauri.conf.json")).unwrap()).unwrap();
    let csp = conf["app"]["security"]["csp"].as_str().expect("a CSP string");
    let directive = |name: &str| -> Vec<String> {
        csp.split(';')
            .map(str::trim)
            .find_map(|d| d.strip_prefix(name).filter(|r| r.starts_with(' ')))
            .unwrap_or_else(|| panic!("CSP has no {name}"))
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(directive("default-src"), ["'self'"]);
    let allowed = ["'self'", "ipc:", "http://ipc.localhost"];
    for src in directive("connect-src") {
        assert!(allowed.contains(&src.as_str()), "connect-src allows {src}");
    }
    for d in ["script-src", "img-src", "font-src", "media-src", "style-src"] {
        for src in directive(d) {
            assert!(
                !src.starts_with("https:") && !src.starts_with("http:") || src.ends_with(".localhost"),
                "{d} allows the remote origin {src}"
            );
            assert!(
                src != "*" && src != "https:" && src != "http:",
                "{d} allows {src}"
            );
        }
    }
    assert!(
        conf["plugins"]["updater"].is_null(),
        "an updater endpoint is configured"
    );
    for e in std::fs::read_dir(r.join("capabilities")).unwrap().flatten() {
        let cap: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap();
        assert!(
            cap["remote"].is_null(),
            "{:?} grants IPC to remote URLs",
            e.path()
        );
        for p in cap["permissions"].as_array().unwrap() {
            let id = p.as_str().or(p["identifier"].as_str()).unwrap_or_default();
            for net in ["http:", "opener:", "shell:", "updater:", "websocket:", "upload:"] {
                assert!(!id.starts_with(net), "{:?} grants {id}", e.path());
            }
        }
    }
}

#[test]
fn the_helper_has_no_cloud_model_and_no_network_entitlement() {
    let r = root();
    let ent = std::fs::read_to_string(r.join("apps/desktop/src-tauri/entitlements.mac.plist")).unwrap();
    for k in [
        "com.apple.security.network.client",
        "com.apple.security.network.server",
        "com.apple.developer.private-cloud-compute",
    ] {
        assert!(!ent.contains(k), "entitlements grant {k}");
    }
    let mut swift = vec![];
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out)
            } else if p.extension().is_some_and(|x| x == "swift") {
                out.push(p)
            }
        }
    }
    walk(&r.join("native/qrtz-helper/Sources"), &mut swift);
    assert!(!swift.is_empty());
    for f in &swift {
        let s = std::fs::read_to_string(f).unwrap();
        for k in [
            "PrivateCloudComputeLanguageModel",
            "URLSession",
            "NWConnection",
            "NWListener",
            "URLRequest",
        ] {
            assert!(!s.contains(k), "{}: {k}", f.display());
        }
    }
}

fn metadata() -> Value {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = std::process::Command::new(cargo)
        .current_dir(root())
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("cargo metadata");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn reqwest_is_behind_qrtz_speech_net_and_only_qrtz_app_turns_it_on() {
    let m = metadata();
    let pkgs = m["packages"].as_array().unwrap();
    let speech = pkgs.iter().find(|p| p["name"] == EXCEPTION.0).unwrap();
    let dep = speech["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == EXCEPTION.1 && d["kind"].is_null())
        .expect("qrtz-speech → reqwest");
    assert_eq!(dep["optional"], true, "reqwest must be optional in qrtz-speech");
    let net = speech["features"][NET_FEATURE]
        .as_array()
        .expect("qrtz-speech has a `net` feature");
    assert!(net.iter().any(|f| f == "dep:reqwest"), "{net:?}");
    for (name, f) in speech["features"].as_object().unwrap() {
        if name != NET_FEATURE {
            let f = f.as_array().unwrap();
            assert!(
                !f.iter().any(|x| x == "dep:reqwest" || x == NET_FEATURE),
                "feature {name} pulls the network: {f:?}"
            );
        }
    }
    let mut users = vec![];
    for p in pkgs {
        for d in p["dependencies"].as_array().unwrap() {
            // Normal (and build) edges only: qrtz-speech's own tests may use `net`.
            if d["name"] != EXCEPTION.0 || d["kind"] == "dev" {
                continue;
            }
            let on = d["features"].as_array().unwrap().iter().any(|f| f == NET_FEATURE)
                || d["uses_default_features"] == true
                    && speech["features"]["default"]
                        .as_array()
                        .is_some_and(|f| f.iter().any(|x| x == NET_FEATURE));
            if on {
                users.push(p["name"].as_str().unwrap().to_owned());
            }
        }
    }
    users.sort();
    assert_eq!(users, NET_FEATURE_USERS, "crates that enable qrtz-speech/net");
}

#[test]
fn qrtz_talk_and_qrtz_lifecycle_build_with_no_http_client() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    for c in SPEECH_WITHOUT_NET {
        // `cargo tree -p` resolves features for that crate alone (what its own build gets).
        let out = std::process::Command::new(&cargo)
            .current_dir(root())
            .args([
                "tree",
                "-p",
                c,
                "-e",
                "normal",
                "--prefix",
                "none",
                "--all-features",
            ])
            .output()
            .expect("cargo tree");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let tree = String::from_utf8_lossy(&out.stdout);
        let net: Vec<&str> = tree
            .lines()
            .filter(|l| {
                let n = l.split_whitespace().next().unwrap_or("");
                NET_CRATES.contains(&n)
            })
            .collect();
        // qrtz-speech --all-features turns `net` on: its own build is checked without it below.
        if *c == "qrtz-speech" {
            continue;
        }
        assert!(net.is_empty(), "{c} reaches the network:\n{}", net.join("\n"));
    }
    let out = std::process::Command::new(&cargo)
        .current_dir(root())
        .args(["tree", "-p", "qrtz-speech", "-e", "normal", "--prefix", "none"])
        .output()
        .expect("cargo tree");
    let tree = String::from_utf8_lossy(&out.stdout);
    assert!(
        !tree
            .lines()
            .any(|l| NET_CRATES.contains(&l.split_whitespace().next().unwrap_or(""))),
        "qrtz-speech without `net` reaches the network:\n{tree}"
    );
}

// ---- ADR 0144: module confinement, M0 ratchets -------------------------------------------------------------------

/// The M0 ownership map (confinement track). Bobs's module manifests replace its owner fields in M1.
fn confinement_map() -> Value {
    let p = root().join("docs/modules/confinement-map.json");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn str_map(v: &Value) -> HashMap<String, String> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
        .collect()
}

fn pairs(v: &Value) -> BTreeSet<(String, String)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p[0].as_str().unwrap().to_owned(),
                p[1].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The modules that own Rust crates. snap (ADR 0159) is Swift only: its host side is in qrtz-app (shell).
const MODULES: &[&str] = &["echo", "nano", "ctrl"];

/// A cross-owner edge that the module split has to remove: anything from one owner into another, except into
/// `shared` (which may itself only depend on `shared`). Same-owner edges are internal to a module.
fn is_boundary(from: &str, to: &str) -> bool {
    from != to && to != "shared"
}

/// Compares the edges found with the allowlist both ways, so the list ratchets: a new edge fails, and an edge
/// that went away fails until the allowlist is trimmed.
fn ratchet(what: &str, found: &BTreeSet<(String, String)>, allowed: &BTreeSet<(String, String)>) {
    let new: Vec<_> = found.difference(allowed).collect();
    let gone: Vec<_> = allowed.difference(found).collect();
    assert!(
        new.is_empty(),
        "{what}: new cross-module edges (ADR 0144: the monolith may only shrink): {new:?}"
    );
    assert!(
        gone.is_empty(),
        "{what}: allowlisted edges no longer exist; trim them from docs/modules/confinement-map.json: {gone:?}"
    );
}

#[test]
fn every_crate_has_one_known_owner() {
    let map = confinement_map();
    let owners: BTreeSet<String> = map["owners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_str().unwrap().to_owned())
        .collect();
    let crates = str_map(&map["crates"]);
    let m = metadata();
    let members: BTreeSet<String> = m["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_owned())
        .collect();
    let mapped: BTreeSet<String> = crates.keys().cloned().collect();
    assert_eq!(
        mapped, members,
        "confinement-map.json crates vs workspace members"
    );
    for (c, o) in &crates {
        assert!(owners.contains(o), "{c}: unknown owner {o}");
        assert_ne!(
            o, "monolith",
            "{c}: a Rust crate can't be newly owned by the monolith"
        );
    }
    for m in MODULES {
        assert!(crates.values().any(|o| o == m), "no crate is owned by {m}");
    }
}

#[test]
fn module_crate_edges_are_the_shrinking_monolith_allowlist() {
    let map = confinement_map();
    let owner = str_map(&map["crates"]);
    let m = metadata();
    let mut found = BTreeSet::new();
    for p in m["packages"].as_array().unwrap() {
        let from = p["name"].as_str().unwrap();
        for d in p["dependencies"].as_array().unwrap() {
            let to = d["name"].as_str().unwrap();
            // Normal and build edges are linked into the product; dev edges are tests only.
            if d["kind"] == "dev" || to == from || !owner.contains_key(to) {
                continue;
            }
            if is_boundary(&owner[from], &owner[to]) {
                found.insert((from.to_owned(), to.to_owned()));
            }
        }
    }
    ratchet("crates", &found, &pairs(&map["monolithCrateEdges"]));
}

#[test]
fn module_and_shared_crates_reach_no_network_crate_in_their_own_build() {
    let map = confinement_map();
    let owner = str_map(&map["crates"]);
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut checked = 0;
    for (c, o) in &owner {
        if o == "shell" {
            continue;
        }
        for t in TARGETS {
            // Features resolved for this crate alone: what its own module binary would get.
            let out = std::process::Command::new(&cargo)
                .current_dir(root())
                .args([
                    "tree",
                    "-p",
                    c,
                    "-e",
                    "normal,build",
                    "--prefix",
                    "none",
                    "--all-features",
                    "--target",
                    t,
                ])
                .output()
                .expect("cargo tree");
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let tree = String::from_utf8_lossy(&out.stdout);
            let net: BTreeSet<&str> = tree
                .lines()
                .filter_map(|l| l.split_whitespace().next())
                .filter(|n| NET_CRATES.contains(n))
                .collect();
            assert!(net.is_empty(), "{c} ({o}) on {t} reaches the network: {net:?}");
            checked += 1;
        }
    }
    assert!(checked >= 10 * TARGETS.len(), "checked only {checked}");
    // And no module or shared crate can switch the download channel on (ADR 0078's `net` users are shell only).
    for u in NET_FEATURE_USERS {
        assert_eq!(
            owner[*u], "shell",
            "{u} enables qrtz-speech/net but isn't the shell"
        );
    }
}

#[test]
fn module_and_shared_crate_sources_name_no_downloader() {
    let map = confinement_map();
    let owner = str_map(&map["crates"]);
    let r = root();
    let mut bad = vec![];
    let mut seen = 0;
    for (c, o) in &owner {
        if o == "shell" {
            continue;
        }
        let dir = r.join("crates").join(c).join("src");
        let mut files = vec![];
        rust_sources(&dir, &mut files);
        seen += files.len();
        for f in &files {
            let rel = f.strip_prefix(&r).unwrap().to_string_lossy().replace('\\', "/");
            for (n, l) in code(f) {
                if [
                    "Downloader",
                    "LocalModels",
                    "speech::download",
                    "speech::update",
                    "local_models",
                    // ADR 0157: the package fetcher is the shell's too.
                    "module_hosting",
                    "HttpTransport",
                    "reqwest",
                    "tungstenite",
                    "ureq::",
                ]
                .iter()
                .any(|k| l.contains(k))
                {
                    bad.push(format!("{rel}:{n} ({o}): {}", l.trim()));
                }
            }
        }
    }
    assert!(seen > 30, "found only {seen} module/shared sources");
    assert!(
        bad.is_empty(),
        "ADR 0144/0157: only the shell downloads (setup.rs, and module_hosting.rs's package fetcher):\n{}",
        bad.join("\n")
    );
}

/// `import X` lines of a Swift file (any attribute prefix), outside `//` comments.
fn swift_imports(src: &str) -> BTreeSet<String> {
    src.lines()
        .map(str::trim_start)
        .filter(|l| !l.starts_with("//"))
        .filter_map(|l| {
            let mut w = l.split_whitespace().skip_while(|w| w.starts_with('@'));
            (w.next() == Some("import")).then(|| w.next()).flatten()
        })
        .map(|m| m.split('.').next().unwrap().to_owned())
        .collect()
}

#[test]
fn swift_target_imports_are_the_shrinking_monolith_allowlist() {
    let map = confinement_map();
    let targets = str_map(&map["swiftTargets"]);
    let mut owner = targets.clone();
    owner.extend(str_map(&map["swiftPackages"]));
    let src = root().join("native/qrtz-helper/Sources");
    let dirs: BTreeSet<String> = std::fs::read_dir(&src)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let mapped: BTreeSet<String> = targets.keys().cloned().collect();
    assert_eq!(
        mapped, dirs,
        "confinement-map.json swiftTargets vs native/qrtz-helper/Sources"
    );
    let monolith: BTreeSet<String> = map["monolithSwiftTargets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let now: BTreeSet<String> = targets
        .iter()
        .filter(|(_, o)| *o == "monolith")
        .map(|(t, _)| t.clone())
        .collect();
    assert_eq!(
        now, monolith,
        "monolith Swift targets may only shrink (trim monolithSwiftTargets with them)"
    );
    let mut found = BTreeSet::new();
    for (t, o) in &targets {
        let mut files = vec![];
        fn walk(d: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out)
                } else if p.extension().is_some_and(|x| x == "swift") {
                    out.push(p)
                }
            }
        }
        walk(&src.join(t), &mut files);
        for f in files {
            for i in swift_imports(&std::fs::read_to_string(&f).unwrap()) {
                // Apple SDK modules aren't in the map; their TCC side is the entitlements lint and the Mac gates.
                if let Some(io) = owner.get(&i) {
                    if &i != t && is_boundary(o, io) && !(o == "monolith" && io == "monolith") {
                        found.insert((t.clone(), i));
                    }
                }
            }
        }
    }
    ratchet("swift imports", &found, &pairs(&map["monolithSwiftImports"]));
}

#[test]
fn swift_import_parser_sees_attributed_imports() {
    let i = swift_imports(
        "import Foundation\n@preconcurrency import FluidAudio\n// import QrtzCtrl\n  import AppKit.NSView\n",
    );
    assert_eq!(
        i,
        BTreeSet::from(["Foundation".into(), "FluidAudio".into(), "AppKit".into()])
    );
}

/// The keys set to `<true/>` in an entitlements plist (a flat dict; arrays and strings count as granted too).
fn granted(plist: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = plist;
    while let Some(i) = rest.find("<key>") {
        rest = &rest[i + 5..];
        let Some(j) = rest.find("</key>") else { break };
        let key = rest[..j].trim().to_owned();
        rest = &rest[j + 6..];
        let v = rest.trim_start();
        if !v.starts_with("<false/>") {
            out.insert(key);
        }
    }
    out
}

/// The TCC entitlement prefixes (the hardened-runtime keys macOS pairs with a privacy grant).
fn is_tcc_entitlement(k: &str) -> bool {
    k.starts_with("com.apple.security.device.")
        || k.starts_with("com.apple.security.personal-information.")
        || k.starts_with("com.apple.security.automation.")
}

/// ADR 0154: the single app's TCC entitlements allowlist, entitlement -> ADR note.
fn app_tcc_allowlist(map: &Value) -> BTreeMap<String, String> {
    map["appTccEntitlements"]
        .as_object()
        .expect("confinement-map.json: appTccEntitlements")
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
        .collect()
}

/// What ADR 0144 (amended by ADR 0154) lets an extension helper's and an inference XPC service's entitlements grant,
/// by file name. Extension helpers share qrtz's identity, so they may hold only the app's TCC allowlist; inference
/// services are sandboxed with no network and no TCC.
fn lint_entitlements(name: &str, plist: &str, map: &Value) -> Result<(), String> {
    let e = &map["entitlements"];
    let class = if name.starts_with("ai.qrtz.inference.") {
        "inference".to_owned()
    } else {
        name.trim_start_matches("ai.qrtz.")
            .trim_end_matches(".entitlements")
            .to_owned()
    };
    let list = |k: &str| -> BTreeSet<String> {
        e[k][&class]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
            .unwrap_or_default()
    };
    let extension = e["extensionHelpers"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(class.as_str())));
    let allowed: BTreeSet<String> = if class == "inference" {
        list("allowedTrue")
    } else if extension {
        app_tcc_allowlist(map).into_keys().collect()
    } else {
        return Err(format!(
            "{name}: no extension helper or inference class `{class}` in the confinement map (ADR 0154)"
        ));
    };
    let g = granted(plist);
    let mut errs = vec![];
    for k in &g {
        let banned = k.starts_with("com.apple.security.network.")
            || k.starts_with("com.apple.developer.private-cloud-compute")
            || (class == "inference" && is_tcc_entitlement(k));
        if banned {
            errs.push(format!("{name}: {class} may not hold {k}"));
        } else if !allowed.contains(k) {
            errs.push(format!("{name}: {k} is not in the allowed set for {class}"));
        }
    }
    for k in list("requiredTrue").difference(&g) {
        errs.push(format!("{name}: {class} must set {k}"));
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

fn entitlements_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        let n = p.file_name().unwrap().to_string_lossy().into_owned();
        if p.is_dir() {
            if !["node_modules", "target", ".build", ".git", "dist"].contains(&n.as_str()) {
                entitlements_files(&p, out);
            }
        } else if n.ends_with(".entitlements") {
            out.push(p);
        }
    }
}

#[test]
fn extension_and_inference_entitlements_stay_inside_their_allowlists() {
    let map = confinement_map();
    let mut files = vec![];
    entitlements_files(&root(), &mut files);
    let mut errs = vec![];
    // ADR 0231 (M6): a network-role crate's entitlements file is linted by its networkRoles entry instead: exactly
    // the listed keys, and never a sandbox, network or TCC key.
    let role_files: BTreeMap<PathBuf, BTreeSet<String>> = network_roles()
        .values()
        .filter_map(|r| {
            let f = r["entitlements"]["file"].as_str()?;
            let keys = r["entitlements"]["keys"]
                .as_array()?
                .iter()
                .filter_map(|k| k.as_str().map(str::to_owned))
                .collect();
            Some((root().join(f), keys))
        })
        .collect();
    for (f, keys) in &role_files {
        let got = granted(&std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{}: {e}", f.display())));
        assert_eq!(&got, keys, "{}: entitlements", f.display());
        for k in &got {
            assert!(
                !k.starts_with("com.apple.security.") && !is_tcc_entitlement(k),
                "{}: {k}",
                f.display()
            );
        }
    }
    for f in &files {
        if role_files
            .keys()
            .any(|p| p.canonicalize().ok() == f.canonicalize().ok())
        {
            continue;
        }
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        // Anything named *.entitlements must follow the naming, so a service can't dodge the lint by name.
        if !name.starts_with("ai.qrtz.") {
            errs.push(format!("{}: entitlements files are named ai.qrtz.<extension>.entitlements or ai.qrtz.inference.<name>.entitlements", f.display()));
            continue;
        }
        if let Err(e) = lint_entitlements(&name, &std::fs::read_to_string(f).unwrap(), &map) {
            errs.push(e);
        }
    }
    // ADR 0154: per-extension files are accepted, never required (none may exist).
    assert!(errs.is_empty(), "{}", errs.join("\n"));
}

#[test]
fn the_entitlements_lint_refuses_network_and_tcc_on_an_inference_service() {
    let map = confinement_map();
    let p = |keys: &[(&str, bool)]| {
        let mut s = String::from("<plist version=\"1.0\"><dict>\n");
        for (k, v) in keys {
            s += &format!("  <key>{k}</key><{}/>\n", if *v { "true" } else { "false" });
        }
        s + "</dict></plist>"
    };
    let sb = ("com.apple.security.app-sandbox", true);
    let voice = "ai.qrtz.inference.voice.entitlements";
    assert!(lint_entitlements(voice, &p(&[sb]), &map).is_ok());
    assert!(lint_entitlements(
        voice,
        &p(&[sb, ("com.apple.security.network.client", false)]),
        &map
    )
    .is_ok());
    for bad in [
        "com.apple.security.network.client",
        "com.apple.security.network.server",
        "com.apple.security.device.audio-input",
        "com.apple.security.device.camera",
        "com.apple.security.personal-information.location",
        "com.apple.security.automation.apple-events",
        "com.apple.security.cs.allow-jit",
    ] {
        let e = lint_entitlements(voice, &p(&[sb, (bad, true)]), &map).unwrap_err();
        assert!(e.contains(bad), "{e}");
    }
    assert!(lint_entitlements(voice, &p(&[]), &map)
        .unwrap_err()
        .contains("app-sandbox"));
    // ADR 0154: extension helpers share qrtz's identity, so any of them may hold what the app may, and nothing more.
    let ae = ("com.apple.security.automation.apple-events", true);
    let mic = ("com.apple.security.device.audio-input", true);
    for m in ["echo", "nano", "ctrl", "snap"] {
        let f = format!("ai.qrtz.{m}.entitlements");
        assert!(lint_entitlements(&f, &p(&[]), &map).is_ok(), "{f}");
        assert!(lint_entitlements(&f, &p(&[mic, ae]), &map).is_ok(), "{f}");
        for bad in [
            "com.apple.security.network.client",
            "com.apple.security.network.server",
            "com.apple.security.device.camera",
            "com.apple.security.personal-information.addressbook",
        ] {
            let e = lint_entitlements(&f, &p(&[mic, (bad, true)]), &map).unwrap_err();
            assert!(e.contains(bad), "{f}: {e}");
        }
    }
    assert!(lint_entitlements("ai.qrtz.shell.entitlements", &p(&[]), &map).is_err());
    assert!(lint_entitlements("ai.qrtz.other.entitlements", &p(&[]), &map).is_err());
}

#[test]
fn the_app_tcc_entitlements_are_the_adr_allowlist() {
    let map = confinement_map();
    let allow = app_tcc_allowlist(&map);
    // Each entry is a TCC entitlement and carries the ADR that admitted it; adding one means writing that ADR.
    for (k, note) in &allow {
        assert!(
            is_tcc_entitlement(k),
            "appTccEntitlements: {k} is not a TCC entitlement"
        );
        let has_adr = note
            .match_indices("ADR ")
            .any(|(i, _)| note[i + 4..].chars().take(4).filter(char::is_ascii_digit).count() == 4);
        assert!(
            has_adr,
            "appTccEntitlements: {k} needs an ADR note (\"ADR NNNN: why\"), got {note:?}"
        );
    }
    // ADR 0154: one qrtz identity, so the app's entitlements are every extension's. They stay inside the allowlist.
    let ent = std::fs::read_to_string(root().join("apps/desktop/src-tauri/entitlements.mac.plist")).unwrap();
    let extra: Vec<String> = granted(&ent)
        .into_iter()
        .filter(|k| is_tcc_entitlement(k) && !allow.contains_key(k))
        .collect();
    assert!(
        extra.is_empty(),
        "entitlements.mac.plist holds TCC entitlements outside appTccEntitlements: {extra:?} (add each with an ADR note)"
    );
    assert!(
        map.get("shellTccEntitlements").is_none(),
        "shellTccEntitlements is retired by ADR 0154; use appTccEntitlements"
    );
}

/// ADR 0212 (superseding ADR 0157 §9's one-exception allowlist): grants are derived from declarations only. Every
/// runnable unit declares its reads (`extension.reads`, `service.models`) and `qrtz_modules::resolve::extension_grants`
/// turns them into grants; no grant logic may name a package or a model. This fails on any package name (a module,
/// one of its dependencies, a pinned model) written as a string literal in the code of the grant logic, and on any
/// allowlist constant coming back.
const GRANT_LOGIC: &[(&str, Option<&str>)] = &[
    ("crates/qrtz-modules/src/resolve.rs", None),
    ("crates/qrtz-modules/src/install.rs", None),
    ("crates/qrtz-modules/src/qrtz_json.rs", None),
    ("crates/qrtz-modules/src/index.rs", None),
    ("crates/qrtz-app/src/local_models.rs", None),
    (
        "crates/qrtz-app/src/installed.rs",
        Some("pub fn read_models_env("),
    ),
];

/// The code of `src` a grant rule looks at: before its `#[cfg(test)]`, comments dropped, or only the item starting
/// at `item` (to its closing brace at column 0).
fn grant_code(src: &str, item: Option<&str>) -> String {
    let body = match item {
        Some(i) => {
            let from = src.find(i).unwrap_or_else(|| panic!("{i} is gone"));
            let rest = &src[from..];
            &rest[..rest.find("\n}\n").map_or(rest.len(), |e| e + 3)]
        }
        None => &src[..src.find("#[cfg(test)]").unwrap_or(src.len())],
    };
    body.lines()
        .map(|l| l.split_once("//").map_or(l, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Package names in `code` written as string literals.
fn named_packages(code: &str, names: &BTreeSet<String>) -> Vec<String> {
    names
        .iter()
        .filter(|n| code.contains(&format!("\"{n}\"")))
        .cloned()
        .collect()
}

#[test]
fn grant_logic_names_no_package_or_model() {
    let r = root();
    let mut names = BTreeSet::new();
    for e in std::fs::read_dir(r.join("modules")).unwrap().flatten() {
        let Ok(t) = std::fs::read_to_string(e.path().join("qrtz.json")) else {
            continue;
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        names.insert(v["name"].as_str().unwrap().to_owned());
        for d in v["dependencies"].as_object().into_iter().flatten() {
            names.insert(d.0.clone());
        }
    }
    let pinned: Value = serde_json::from_str(
        &std::fs::read_to_string(r.join("crates/qrtz-app/src/local_models.json")).unwrap(),
    )
    .unwrap();
    for s in pinned["sets"].as_array().unwrap() {
        names.insert(s["package"].as_str().unwrap().to_owned());
    }
    assert!(
        names.contains("model.laya") && names.contains("model.localvqe") && names.contains("nano"),
        "{names:?}"
    );
    let mut bad = vec![];
    for (f, item) in GRANT_LOGIC {
        let code = grant_code(&std::fs::read_to_string(r.join(f)).unwrap(), *item);
        for n in named_packages(&code, &names) {
            bad.push(format!("{f}: \"{n}\""));
        }
        if code.contains("EXTENSION_READS") {
            bad.push(format!("{f}: an EXTENSION_READS allowlist"));
        }
    }
    assert!(
        bad.is_empty(),
        "grant logic names packages (declare reads in qrtz.json instead): {bad:?}"
    );
    // The checker itself.
    let fake = "fn grants() { if ext == \"nano\" && m == \"model.laya\" { ok() } }\n#[cfg(test)]\nmod t { \"ctrl\" }";
    assert_eq!(
        named_packages(&grant_code(fake, None), &names),
        ["model.laya", "nano"]
    );
    assert!(named_packages(&grant_code("// \"nano\" in a comment\nfn f() {}", None), &names).is_empty());
}

/// ADR 0162, ADR 0169: every executable Tauri bundles into Contents/MacOS (tauri.macos.conf.json externalBin) is in
/// the confinement map's bundledCode, scripts/release/sign-mac.sh signs exactly those with the identifier and
/// entitlements listed there, each entitlements file passes the lint, and an extension's bundled helper signs as the
/// identifier its lock entry pins. (Since ADR 0169 that's only the `qrtz` command; module code is moduleCode.)
#[test]
fn every_bundled_executable_is_signed_as_the_confinement_map_says() {
    let map = confinement_map();
    let code = map["bundledCode"].as_object().expect("bundledCode");
    let mapped: BTreeMap<String, (String, String)> = code
        .iter()
        .filter(|(_, v)| v.is_object())
        .map(|(k, v)| {
            let ent = match &v["entitlements"] {
                Value::Null => "none".to_owned(),
                e => e.as_str().unwrap().to_owned(),
            };
            (k.clone(), (v["identifier"].as_str().unwrap().to_owned(), ent))
        })
        .collect();
    // The externalBin list.
    let conf: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("apps/desktop/src-tauri/tauri.macos.conf.json")).unwrap(),
    )
    .unwrap();
    let bins: BTreeSet<String> = conf["bundle"]["externalBin"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_str().unwrap().trim_start_matches("binaries/").to_owned())
        .collect();
    assert_eq!(
        bins,
        mapped.keys().cloned().collect(),
        "externalBin and confinement-map bundledCode differ"
    );
    // The signing script's table.
    let script = std::fs::read_to_string(root().join("scripts/release/sign-mac.sh")).unwrap();
    let table = script
        .split("NESTED=\"")
        .nth(1)
        .and_then(|t| t.split('"').next())
        .expect("NESTED table in sign-mac.sh");
    let signed: BTreeMap<String, (String, String)> = table
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            assert_eq!(f.len(), 3, "sign-mac.sh NESTED line: {l}");
            (f[0].to_owned(), (f[1].to_owned(), f[2].to_owned()))
        })
        .collect();
    assert_eq!(signed, mapped, "sign-mac.sh signs differently from bundledCode");
    let owners: BTreeSet<&str> = map["owners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_str().unwrap())
        .collect();
    let lock: Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("modules/modules.lock.json")).unwrap())
            .unwrap();
    for (name, (id, ent)) in &mapped {
        let owner = code[name]["owner"].as_str().unwrap();
        assert!(owners.contains(owner), "{name}: unknown owner {owner}");
        if ent != "app" && ent != "none" {
            let file = Path::new(ent).file_name().unwrap().to_str().unwrap();
            let plist = std::fs::read_to_string(root().join(ent)).unwrap_or_else(|e| panic!("{ent}: {e}"));
            lint_entitlements(file, &plist, &map).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        // A module's bundled helper signs as its lock entry's bundle id (what its designated requirement names).
        if let Some(m) = lock["modules"].get(owner) {
            assert_eq!(
                m["code"]["bundleId"].as_str(),
                Some(id.as_str()),
                "{name}: identifier differs from {owner}'s lock entry"
            );
        }
    }
    assert_eq!(
        code["qrtz"]["entitlements"],
        Value::Null,
        "the qrtz command carries no entitlement"
    );
}

/// ADR 0169 (Bozniak's finding): no module code is bundled, and each module's executable is signed with the
/// identifier its lock entry's designated requirement names. confinement-map.json moduleCode is the table;
/// scripts/modules/build-packages.sh signs exactly that; the lock (modules.lock.json), module.json and the package's
/// qrtz.json all agree with it; and the launch entry is the package's own executable, never the qrtz-helper monolith.
#[test]
fn every_lock_requirement_names_the_identifier_its_module_is_signed_with() {
    let map = confinement_map();
    let code = map["moduleCode"].as_object().expect("moduleCode");
    assert_eq!(code["teamId"].as_str(), Some(RELEASE_TEAM));
    let bundled = map["bundledCode"].as_object().unwrap();
    let table_of = |t: &serde_json::Map<String, Value>| -> BTreeMap<String, (String, String)> {
        t.iter()
            .filter(|(k, v)| v.is_object() && k.as_str() != "devHelper" && k.as_str() != "qrtz")
            .map(|(k, v)| {
                (
                    k.clone(),
                    (
                        v["identifier"].as_str().unwrap().to_owned(),
                        v["entitlements"].as_str().unwrap().to_owned(),
                    ),
                )
            })
            .collect()
    };
    // ADR 0174: echo's executable is bundled (bundledCode); nano, ctrl and snap are packages (moduleCode).
    let downloadable = table_of(code);
    let in_app = table_of(bundled);
    assert_eq!(
        downloadable.keys().map(String::as_str).collect::<Vec<_>>(),
        ["qrtz-ctrl", "qrtz-nano", "qrtz-snap"],
        "moduleCode names exactly the three downloadable modules"
    );
    assert_eq!(
        in_app.keys().map(String::as_str).collect::<Vec<_>>(),
        ["qrtz-echo"],
        "bundledCode carries exactly echo's module code"
    );
    let mut mapped = downloadable.clone();
    mapped.extend(in_app.clone());
    // Not vacuous (902bcab review): every module qrtz ships is in one of the tables, so dropping one from the maps,
    // the signing scripts and the lock together still fails here.
    assert_eq!(
        mapped.keys().map(String::as_str).collect::<Vec<_>>(),
        ["qrtz-ctrl", "qrtz-echo", "qrtz-nano", "qrtz-snap"],
        "the confinement map names exactly the four modules"
    );
    // A downloadable module (and the dev monolith) is never bundled; a bundled one is never a package.
    for name in downloadable
        .keys()
        .chain(std::iter::once(&"qrtz-helper".to_owned()))
    {
        assert!(
            !bundled.contains_key(name),
            "{name} is bundled in qrtz.app (ADR 0169)"
        );
    }
    let bundled_ids = |s: &str| -> Vec<String> {
        s.split("BUNDLED_EXTENSIONS: &[&str] = &[")
            .nth(1)
            .and_then(|t| t.split(']').next())
            .unwrap()
            .split(',')
            .map(|x| x.trim().trim_matches('"').to_owned())
            .filter(|x| !x.is_empty())
            .collect()
    };
    let lib = std::fs::read_to_string(root().join("crates/qrtz-modules/src/lib.rs")).unwrap();
    assert_eq!(
        bundled_ids(&lib),
        in_app
            .keys()
            .map(|n| n.trim_start_matches("qrtz-").to_owned())
            .collect::<Vec<_>>(),
        "qrtz_modules::BUNDLED_EXTENSIONS and bundledCode differ"
    );
    // The signing script's table.
    let script = std::fs::read_to_string(root().join("scripts/modules/build-packages.sh")).unwrap();
    let table = script
        .split("MODULES=\"")
        .nth(1)
        .and_then(|t| t.split('"').next())
        .expect("MODULES table in build-packages.sh");
    let signed: BTreeMap<String, (String, String)> = table
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            assert_eq!(f.len(), 3, "build-packages.sh MODULES line: {l}");
            (format!("qrtz-{}", f[0]), (f[1].to_owned(), f[2].to_owned()))
        })
        .collect();
    assert_eq!(
        signed, downloadable,
        "build-packages.sh signs differently from moduleCode"
    );
    let lock: Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("modules/modules.lock.json")).unwrap())
            .unwrap();
    let locked: BTreeSet<String> = lock["modules"].as_object().unwrap().keys().cloned().collect();
    let owners: BTreeSet<String> = mapped
        .keys()
        .map(|n| {
            code.get(n).unwrap_or_else(|| &bundled[n])["owner"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        locked, owners,
        "every locked module has its executable in moduleCode or bundledCode"
    );
    let read = |p: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(root().join(p)).unwrap())
            .unwrap_or_else(|e| panic!("{p}: {e}"))
    };
    for (name, (id, ent)) in &mapped {
        let is_bundled = in_app.contains_key(name);
        let owner = code.get(name).unwrap_or_else(|| &bundled[name])["owner"]
            .as_str()
            .unwrap();
        assert_eq!(name, &format!("qrtz-{owner}"));
        let dr_id = |dr: &str| {
            dr.strip_prefix("identifier \"")
                .and_then(|r| r.split('"').next())
                .map(str::to_owned)
        };
        let l = &lock["modules"][owner]["code"];
        assert_eq!(
            l["bundleId"].as_str(),
            Some(id.as_str()),
            "{owner}: lock bundleId"
        );
        let dr = l["designatedRequirement"].as_str().unwrap();
        assert_eq!(
            dr_id(dr).as_deref(),
            Some(id.as_str()),
            "{owner}: the lock's DR names another identifier"
        );
        // The whole requirement, not just its identifier: Developer ID (anchor apple generic, the Developer ID CA
        // and leaf), team MR26CCZ496 (qrtz_modules::manifest::designated_requirement, what the installer and the
        // launcher require).
        assert_eq!(
            dr,
            format!(
                "identifier \"{id}\" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = \"{RELEASE_TEAM}\""
            ),
            "{owner}: the lock's DR"
        );
        assert_eq!(l["teamId"].as_str(), Some(RELEASE_TEAM), "{owner}: lock teamId");
        let m = read(&format!("modules/{owner}/module.json"));
        assert_eq!(
            m["code"]["bundleId"].as_str(),
            Some(id.as_str()),
            "{owner}: module.json bundleId"
        );
        assert_eq!(
            m["code"]["designatedRequirement"].as_str(),
            Some(dr),
            "{owner}: module.json DR"
        );
        if is_bundled {
            // ADR 0174: qrtz.app/Contents/MacOS/qrtz-<id>, launched under the bundled manifest's requirement (the
            // one checked above, compiled into the signed app), with no package and no notarization record.
            assert_eq!(m["launch"]["kind"], "bundled-helper", "{owner}: launch.kind");
            assert_eq!(
                m["launch"]["entry"].as_str(),
                Some(name.as_str()),
                "{owner}: launch.entry"
            );
            let tauri: Value = read("apps/desktop/src-tauri/tauri.macos.conf.json");
            assert!(
                tauri["bundle"]["externalBin"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|b| b.as_str() == Some(&format!("binaries/{name}"))),
                "{name} is an externalBin"
            );
        } else {
            assert_eq!(m["launch"]["kind"], "package", "{owner}: launch.kind");
            let q = read(&format!("modules/{owner}/qrtz.json"));
            let entry = q["extension"]["entry"].as_str().unwrap();
            assert_eq!(entry, format!("bin/{name}"), "{owner}: qrtz.json extension.entry");
            assert_eq!(
                m["launch"]["entry"].as_str(),
                Some(entry),
                "{owner}: launch.entry"
            );
        }
        // What the package installer pins for an extension (qrtz_modules::qrtz_json::identifier_for).
        assert_eq!(
            id,
            &format!("ai.qrtz.{owner}"),
            "{owner}: the installer pins another identifier"
        );
        let file = Path::new(ent).file_name().unwrap().to_str().unwrap();
        assert_eq!(file, format!("{id}.entitlements"));
        let plist = std::fs::read_to_string(root().join(ent)).unwrap_or_else(|e| panic!("{ent}: {e}"));
        lint_entitlements(file, &plist, &map).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    // snap (ADR 0159): no entitlement at all.
    let snap = std::fs::read_to_string(root().join(&mapped["qrtz-snap"].1)).unwrap();
    assert!(
        granted(&snap).is_empty() && !snap.contains("<key>"),
        "qrtz-snap carries no entitlement"
    );
    // The dev monolith has its own identifier, which nothing pins.
    let dev = &code["devHelper"];
    assert_eq!(dev["identifier"], "ai.qrtz.helper");
    let all = std::fs::read_to_string(root().join("modules/modules.lock.json")).unwrap();
    assert!(!all.contains("ai.qrtz.helper") && !all.contains("\\\"qrtz-helper\\\""));
    let dev_sign = std::fs::read_to_string(root().join("scripts/dev/sign.sh")).unwrap();
    assert!(dev_sign.contains("ai.qrtz.helper") && !dev_sign.contains("sign qrtz-helper qrtz-helper"));
}

/// The Developer ID team every release path pins: `Developer ID Application: Jared Grimes (MR26CCZ496)`. Fails if a
/// bundled manifest, the bundled lockfile, the confinement map, the store's service packages or a release script names
/// another team (the placeholder `0000000000` included). Test fixtures keep their fake team; they aren't read here.
const RELEASE_TEAM: &str = "MR26CCZ496";
const RELEASE_IDENTITY: &str = "Developer ID Application: Jared Grimes (MR26CCZ496)";

fn team_ids(v: &Value, at: &str, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                let here = format!("{at}.{k}");
                if k == "teamId" {
                    out.push((here.clone(), x.as_str().unwrap_or("<not a string>").to_owned()));
                }
                if k == "designatedRequirement" {
                    let dr = x.as_str().unwrap_or("");
                    let tail = format!("certificate leaf[subject.OU] = \"{RELEASE_TEAM}\"");
                    out.push((
                        here.clone(),
                        if dr.ends_with(&tail) {
                            RELEASE_TEAM.to_owned()
                        } else {
                            dr.to_owned()
                        },
                    ));
                }
                team_ids(x, &here, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                team_ids(x, &format!("{at}[{i}]"), out);
            }
        }
        _ => {}
    }
}

fn release_team_findings() -> Vec<(String, String)> {
    let mut files = vec!["modules/modules.lock.json".to_owned()];
    for d in std::fs::read_dir(root().join("modules")).unwrap() {
        let d = d.unwrap().path();
        for f in ["module.json", "qrtz.json"] {
            if d.join(f).is_file() {
                files.push(
                    d.join(f)
                        .strip_prefix(root())
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                );
            }
        }
    }
    // ADR 0174: v1 publishes no service package (inference.voice was cut), so the dir may be absent.
    for d in std::fs::read_dir(root().join("tools/qrtz-publish/packages"))
        .into_iter()
        .flatten()
    {
        let f = d.unwrap().path().join("qrtz.json");
        if f.is_file() {
            files.push(f.strip_prefix(root()).unwrap().to_str().unwrap().to_owned());
        }
    }
    let mut out = vec![];
    for f in &files {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(root().join(f)).unwrap())
            .unwrap_or_else(|e| panic!("{f}: {e}"));
        team_ids(&v, f, &mut out);
    }
    let map = confinement_map();
    out.push((
        "confinement-map.json bundledCode.teamId".into(),
        map["bundledCode"]["teamId"]
            .as_str()
            .unwrap_or("<missing>")
            .to_owned(),
    ));
    out
}

#[test]
fn every_release_path_pins_jareds_developer_id_team() {
    let found = release_team_findings();
    // The four bundled extensions' code identities at least (manifest and lock).
    let code = found
        .iter()
        .filter(|(at, _)| at.ends_with(".code.teamId"))
        .count();
    assert!(
        code >= 8,
        "expected echo, nano, ctrl and snap in manifests and lock, found {code}"
    );
    let wrong: Vec<_> = found.iter().filter(|(_, t)| t != RELEASE_TEAM).collect();
    assert!(
        wrong.is_empty(),
        "release team must be {RELEASE_TEAM}: {wrong:#?}"
    );

    let manifest_rs = std::fs::read_to_string(root().join("crates/qrtz-modules/src/manifest.rs")).unwrap();
    assert!(
        manifest_rs.contains(&format!("pub const RELEASE_TEAM_ID: &str = \"{RELEASE_TEAM}\";")),
        "qrtz_modules::manifest::RELEASE_TEAM_ID"
    );
    let local_models = std::fs::read_to_string(root().join("crates/qrtz-app/src/local_models.rs")).unwrap();
    assert!(
        !local_models.contains("PLACEHOLDER_TEAM_ID"),
        "local_models.rs pins models with the placeholder team"
    );

    // The release scripts default to the release identity and team; QRTZ_SIGN_ID / APPLE_SIGNING_IDENTITY and
    // QRTZ_TEAM_ID still override them.
    for (script, needles) in [
        (
            "scripts/release/sign-notarize.sh",
            vec![
                format!("QRTZ_TEAM_ID:-{RELEASE_TEAM}"),
                RELEASE_IDENTITY.to_owned(),
                "QRTZ_NOTARY_PROFILE:-qrtz-notary".to_owned(),
            ],
        ),
        (
            "scripts/release/sign-mac.sh",
            vec![
                format!("QRTZ_TEAM_ID:-{RELEASE_TEAM}"),
                RELEASE_IDENTITY.to_owned(),
                "QRTZ_NOTARY_PROFILE:-qrtz-notary".to_owned(),
            ],
        ),
        (
            "scripts/modules/publish.sh",
            vec![format!("QRTZ_TEAM_ID:-{RELEASE_TEAM}")],
        ),
    ] {
        let s = std::fs::read_to_string(root().join(script)).unwrap();
        for n in &needles {
            assert!(s.contains(n.as_str()), "{script} must default to {n}");
        }
        for (i, l) in s.lines().enumerate() {
            // The only placeholder mentions allowed are the refusals.
            if l.contains("0000000000") {
                assert!(
                    l.contains("!= 0000000000") || l.contains("placeholder"),
                    "{script}:{}: the placeholder team on a release path: {l}",
                    i + 1
                );
            }
        }
    }
    // `bun run make` must not notarize (sign-mac.sh re-signs the nested code first): no identity or Apple ID in the
    // Tauri config.
    for conf in [
        "apps/desktop/src-tauri/tauri.conf.json",
        "apps/desktop/src-tauri/tauri.macos.conf.json",
    ] {
        let p = root().join(conf);
        if p.is_file() {
            let s = std::fs::read_to_string(&p).unwrap();
            assert!(
                !s.contains("signingIdentity") && !s.contains(RELEASE_TEAM),
                "{conf}: signing belongs to scripts/release"
            );
        }
    }
}

#[test]
fn the_release_team_check_catches_the_placeholder() {
    let mut out = vec![];
    team_ids(
        &serde_json::json!({"modules": {"echo": {"code": {"teamId": "0000000000",
            "designatedRequirement": "identifier \"ai.qrtz.echo\" and certificate leaf[subject.OU] = \"0000000000\""}}}}),
        "x",
        &mut out,
    );
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|(_, t)| t != RELEASE_TEAM), "{out:?}");
}

/// ADR 0171: modules.qrtz.ai never hosts model bytes. The app reaches Hugging Face only (the pinned model mirror),
/// the publish script uploads only `v1/` of its output (qrtz-publish keeps model files beside it, in
/// `<out>.model-blobs`, for offline packs) and can write its output without any upload (no rclone, no R2 token).
#[test]
fn model_files_come_from_hugging_face_and_are_never_uploaded() {
    let hosts: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("crates/qrtz-app/src/module_hosts.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        hosts["modelMirrors"],
        serde_json::json!(["https://huggingface.co"])
    );
    let s = std::fs::read_to_string(root().join("scripts/modules/publish.sh")).unwrap();
    let copies: Vec<&str> = s.lines().filter(|l| l.contains("rclone copy")).collect();
    assert!(!copies.is_empty());
    for l in &copies {
        assert!(
            l.contains("rclone copy \"$out/v1\""),
            "publish.sh uploads only $out/v1: {l}"
        );
        assert!(!l.contains("model-blobs"), "{l}");
    }
    assert!(
        !s.contains("model-blobs\" \""),
        "publish.sh never names the model blob dir as a source"
    );
    for n in [
        "--out-only",
        "upload-list",
        "model-files",
        "curl -fL --retry 5 --retry-delay 2 -C -",
    ] {
        assert!(s.contains(n), "publish.sh: {n}");
    }
    // The client fetcher: a blob with signed URLs never asks a module host.
    let f = std::fs::read_to_string(root().join("crates/qrtz-modules/src/fetch.rs")).unwrap();
    assert!(f.contains("ADR 0171"), "fetch.rs: the model-source rule");
}

// ---- ADR 0231 amendment 1: network-role crates -------------------------------------------------------------------

#[test]
fn network_role_crates_are_separate_processes_with_only_their_declared_network_crates() {
    let roles = network_roles();
    let owner = str_map(&confinement_map()["crates"]);
    assert!(
        !roles.is_empty(),
        "networkRoles.crates is empty: drop this test and the role rule"
    );
    for t in TARGETS {
        let g = graph(t);
        for (c, rule) in &roles {
            let id = g.id(c);
            assert!(
                rule["role"].as_str().is_some_and(|r| !r.is_empty()),
                "{c}: no role"
            );
            assert!(rule["adr"].as_str().is_some_and(|r| !r.is_empty()), "{c}: no ADR");
            // No workspace crate links it: it only ever runs as its own process.
            for m in &g.members {
                assert!(
                    !g.normal[m].contains(&id),
                    "{t}: {} links the network-role crate {c}",
                    g.name[m]
                );
            }
            // Its direct network crates are exactly the declared ones.
            let want: BTreeSet<String> = rule["netCrates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            let got: BTreeSet<String> = g.normal[&id]
                .iter()
                .map(|d| g.name[d].clone())
                .filter(|n| NET_CRATES.contains(&n.as_str()))
                .collect();
            assert_eq!(got, want, "{t}: {c}'s direct network crates");
            // Its workspace dependencies are shared-owned only (no vault, notes, transcripts, keys, app).
            for d in &g.normal[&id] {
                let n = &g.name[d];
                if let Some(o) = owner.get(n) {
                    assert_eq!(o, "shared", "{t}: {c} depends on {n} ({o})");
                }
            }
        }
    }
}

#[test]
fn network_role_crate_sources_name_no_url_outside_their_egress() {
    let r = root();
    for (c, rule) in network_roles() {
        let egress: Vec<String> = rule["egress"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(
            egress.iter().all(|e| e.starts_with("https://")),
            "{c}: egress must be https: {egress:?}"
        );
        let mut files = vec![];
        rust_sources(&r.join("crates").join(&c).join("src"), &mut files);
        assert!(!files.is_empty(), "{c}: no sources");
        let mut bad = vec![];
        for f in &files {
            let rel = f.strip_prefix(&r).unwrap().to_string_lossy().replace('\\', "/");
            for (n, l) in code(f) {
                for (i, _) in l.match_indices("://") {
                    let start = l[..i]
                        .rfind(|ch: char| !ch.is_ascii_alphanumeric())
                        .map_or(0, |x| x + 1);
                    let url = &l[start..];
                    if !egress.iter().any(|e| url.starts_with(e.as_str())) {
                        bad.push(format!("{rel}:{n}: {}", l.trim()));
                    }
                }
                // Sockets stay out of it too: the HTTP client is its only way out.
                for k in [
                    "TcpStream",
                    "TcpListener",
                    "UdpSocket",
                    "TcpSocket",
                    "ToSocketAddrs",
                ] {
                    if l.contains(k) {
                        bad.push(format!("{rel}:{n}: {}", l.trim()));
                    }
                }
            }
        }
        assert!(
            bad.is_empty(),
            "{c} names a URL outside its egress:\n{}",
            bad.join("\n")
        );
    }
}

#[test]
fn qrtz_app_own_graph_reaches_no_network_crate_but_the_speech_channel() {
    let roles = network_roles();
    for t in TARGETS {
        let g = graph(t);
        let cut = g.reach("qrtz-app", true);
        assert!(
            cut.is_empty(),
            "{t}: qrtz-app reaches the network:\n{}",
            cut.join("\n")
        );
        // And no network-role crate is anywhere in its graph (it is spawned, never linked).
        let start = g.id("qrtz-app");
        let mut seen = BTreeSet::from([start.clone()]);
        let mut q = VecDeque::from([start]);
        while let Some(at) = q.pop_front() {
            for d in &g.normal[&at] {
                assert!(
                    !roles.contains_key(&g.name[d]),
                    "{t}: qrtz-app reaches {}",
                    g.name[d]
                );
                if seen.insert(d.clone()) {
                    q.push_back(d.clone());
                }
            }
        }
    }
}

#[test]
fn every_network_role_names_an_adr_that_names_its_crate() {
    let adrs = root().join("docs/adr");
    for (c, rule) in network_roles() {
        let n = rule["adr"].as_str().unwrap_or_else(|| panic!("{c}: no adr"));
        let files: Vec<PathBuf> = std::fs::read_dir(&adrs)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|f| f.to_str())
                    .is_some_and(|f| f.starts_with(&format!("{n}-")) && f.ends_with(".md"))
            })
            .collect();
        assert_eq!(files.len(), 1, "{c}: ADR {n} is not exactly one file in docs/adr");
        let text = std::fs::read_to_string(&files[0]).unwrap();
        assert!(
            text.contains(c.as_str()),
            "{c}: ADR {n} ({}) never names the crate",
            files[0].display()
        );
    }
}

#[test]
fn codeowners_gates_the_confinement_and_release_files() {
    let s = std::fs::read_to_string(root().join(".github/CODEOWNERS")).unwrap();
    for f in [
        "/docs/modules/confinement-map.json",
        "/crates/qrtz-lifecycle/tests/offline_graph.rs",
        "/scripts/release/verify-bundle.sh",
        "/contracts/ipc-contract.json",
    ] {
        assert!(
            s.lines()
                .any(|l| l.split_whitespace().next() == Some(f) && l.contains("@jlgrimes")),
            "CODEOWNERS: {f}"
        );
    }
}

/// Bozniak L: no workspace crate's build dependencies (build scripts and proc macros run at build time, with the
/// network the build machine has) reach a network crate, on any target, through normal or build edges.
#[test]
fn no_crate_build_dependency_reaches_a_network_crate() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    for t in TARGETS {
        let out = std::process::Command::new(&cargo)
            .current_dir(root())
            .args(["metadata", "--format-version", "1", "--filter-platform", t])
            .output()
            .expect("cargo metadata");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let m: Value = serde_json::from_slice(&out.stdout).unwrap();
        let name: HashMap<String, String> = m["packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["id"].as_str().unwrap().to_owned(),
                    p["name"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        // id -> (dep id, is build edge) over normal and build edges.
        let mut edges: HashMap<String, Vec<(String, bool)>> = HashMap::new();
        for n in m["resolve"]["nodes"].as_array().unwrap() {
            let v = n["deps"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|d| {
                    let kinds = d["dep_kinds"].as_array().unwrap();
                    let build = kinds.iter().any(|k| k["kind"] == "build");
                    let normal = kinds.iter().any(|k| k["kind"].is_null());
                    (build || normal).then(|| (d["pkg"].as_str().unwrap().to_owned(), build))
                })
                .collect();
            edges.insert(n["id"].as_str().unwrap().to_owned(), v);
        }
        let mut bad = vec![];
        let mut checked = 0;
        for member in m["workspace_members"].as_array().unwrap() {
            let member = member.as_str().unwrap();
            for (b, is_build) in &edges[member] {
                if !is_build {
                    continue;
                }
                checked += 1;
                let mut seen = BTreeSet::from([b.clone()]);
                let mut q = VecDeque::from([b.clone()]);
                while let Some(at) = q.pop_front() {
                    if NET_CRATES.contains(&name[&at].as_str()) {
                        bad.push(format!(
                            "{}: build-dependency {} reaches {}",
                            name[member], name[b], name[&at]
                        ));
                    }
                    for (d, _) in &edges[&at] {
                        if seen.insert(d.clone()) {
                            q.push_back(d.clone());
                        }
                    }
                }
            }
        }
        assert!(
            checked > 0,
            "{t}: no build dependencies found (the check would be vacuous)"
        );
        assert!(bad.is_empty(), "{t}:\n{}", bad.join("\n"));
    }
}

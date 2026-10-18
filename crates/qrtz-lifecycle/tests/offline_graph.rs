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
//! 5. ADR 0079: the downloader's only callers are qrtz-app's `setup.rs` (the signed module) and `local_models.rs`
//!    (the helper's pinned models), and the pinned models come only from `https://huggingface.co` at fixed revisions.
//!    ctrl's Laya model (ADR 0116) goes through `local_models.rs` too: `ctrl_model.rs` names no downloader, and its
//!    manifest is pinned the same way (or a placeholder, which is refused before any request).
//!    ADR 0153: the module installer's transport (`module_hosting.rs`) is the third caller, to the hosts pinned in
//!    `module_hosts.json` only (`https://modules.qrtz.ai` and Backblaze B2 mirrors).
//!    ADR 0157: the package manager's one fetcher (`module_hosting.rs`) also serves the pinned models: a model
//!    file's signed tree may name a mirror URL, reachable only under `modelMirrors` (exactly
//!    `https://huggingface.co`). `local_models.rs` installs through it and names no downloader of its own.
//!
//! 6. ADR 0144 (module confinement, M0): every crate and Swift target has an owner in `docs/modules/confinement-map.json`
//!    (shell / shared / echo / nano / ctrl / voice / monolith); module-owned and shared crates reach no network crate in
//!    their own build and name no downloader; cross-owner edges (the monolith) are an exact allowlist that may only
//!    shrink; module and inference XPC entitlements files are linted by name.
//!    ADR 0154 (one qrtz identity): extension helper entitlements (`ai.qrtz.<echo|nano|ctrl>`) are accepted, not
//!    required, and may hold only the app's TCC allowlist (`appTccEntitlements`, each entry with an ADR note); inference
//!    XPC services stay strict (app-sandbox, no network/device/personal-information/automation).
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

#[test]
fn only_qrtz_speech_names_a_network_crate_directly() {
    let g = graph("aarch64-apple-darwin");
    let mut direct = vec![];
    for m in &g.members {
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
    let mut bad = vec![];
    for f in &files {
        let rel = f.strip_prefix(&r).unwrap().to_string_lossy().replace('\\', "/");
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

#[test]
fn only_setup_and_the_pinned_local_models_call_the_downloader() {
    let r = root();
    let mut files = vec![];
    rust_sources(&r.join("crates"), &mut files);
    rust_sources(&r.join("apps/desktop/src-tauri/src"), &mut files);
    let allowed = [
        "crates/qrtz-app/src/setup.rs",
        "crates/qrtz-app/src/local_models.rs",
        // ADR 0153: the module installer's HTTPS transport, to the pinned module hosts only (checked below).
        "crates/qrtz-app/src/module_hosting.rs",
    ];
    let mut bad = vec![];
    for f in &files {
        let rel = f.strip_prefix(&r).unwrap().to_string_lossy().replace('\\', "/");
        if rel.starts_with("crates/qrtz-speech/") || rel.contains("/tests/") || rel.contains("/examples/") {
            continue;
        }
        for (n, l) in code(f) {
            if (l.contains("Downloader") || l.contains("speech::download"))
                && !allowed.contains(&rel.as_str())
            {
                bad.push(format!("{rel}:{n}: {}", l.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "downloader used outside ADR 0079's callers:\n{}",
        bad.join("\n")
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
    // ADR 0153: the module hosts are exactly the primary (Cloudflare R2 behind modules.qrtz.ai) and Backblaze B2
    // mirrors, https, no path, query or credentials; adding a host changes this test and the ADR.
    let hosts: Value = serde_json::from_str(
        &std::fs::read_to_string(r.join("crates/qrtz-app/src/module_hosts.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        hosts["primary"], "https://modules.qrtz.ai",
        "module_hosts.json primary"
    );
    let mirrors = hosts["mirrors"].as_array().unwrap();
    assert!(mirrors.len() <= 2, "module_hosts.json: at most two mirrors");
    for m in mirrors {
        let m = m.as_str().unwrap();
        let host = m.strip_prefix("https://").unwrap_or_else(|| panic!("{m}: https"));
        assert!(
            host.ends_with(".backblazeb2.com")
                && host
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-'),
            "{m}: a Backblaze B2 host and nothing after it"
        );
    }
    // ADR 0157: the only extra mirror host a signed tree may point at (the pinned models' upstream).
    assert_eq!(
        hosts["modelMirrors"],
        serde_json::json!(["https://huggingface.co"]),
        "module_hosts.json modelMirrors"
    );
    let pin = &hosts["root1Sha256"];
    assert!(
        pin.is_null()
            || pin
                .as_str()
                .is_some_and(|p| p.len() == 64 && p.bytes().all(|b| b.is_ascii_hexdigit())),
        "root1Sha256: null or a sha256"
    );
    // ADR 0116: ctrl's model is fetched through the pinned path, not a downloader of its own.
    let ctrl = std::fs::read_to_string(r.join("crates/qrtz-app/src/ctrl_model.rs")).unwrap();
    assert!(
        ctrl.contains("LocalModels"),
        "ctrl_model.rs downloads through local_models"
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
        "ADR 0144: only the shell downloads (download.rs via LocalModels):\n{}",
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
    for f in &files {
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
    for m in ["echo", "nano", "ctrl"] {
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

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
//!
//! A new network path has to change this test and its ADR, on purpose.
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, VecDeque};
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

/// The one allowed edge into NET_CRATES (ADR 0046 §4, ADR 0078).
const EXCEPTION: (&str, &str) = ("qrtz-speech", "reqwest");

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

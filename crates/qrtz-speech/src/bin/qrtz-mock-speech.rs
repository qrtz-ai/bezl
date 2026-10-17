//! The mock "qrtz speech" module (ADR 0046): speaks protocol v1 on the inherited fds (3 control in, 4 results out,
//! 5 mic PCM, 6 system PCM, 7.. model files). Its behaviour comes from its first model file (fd 7), a JSON object,
//! so tests pick it by building a signed bundle, exactly as a real module gets its weights:
//!
//!   {"version": "1.0.0", "mode": "normal", "say": "the quick brown fox"}
//!
//! modes: normal | probe (`probeUnsafe: false` skips the exec and ptrace probes, for unconfined runs) (normal, plus a `log` line with confinement probes and the open fds) | never-reads (never
//! reads its audio fds: the host must not block, N77) | crash (exits after hello) | silent (never says hello) |
//! garbage (invalid lines after start).
//!
//! normal: one final utterance per full second of audio on a channel, the next word of `say`; on `finish`, a
//! channel that got audio but no utterance yet gets all of `say` spread over its audio; then `finished`.
use serde_json::{json, Value};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::FromRawFd;

const BYTES_PER_MS: u64 = 32; // 16 kHz, 16-bit mono

fn errno_name(r: i64) -> String {
    if r >= 0 {
        return "ok".into();
    }
    let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    match e {
        libc::EPERM => "EPERM".into(),
        libc::EACCES => "EACCES".into(),
        libc::ENOENT => "ENOENT".into(),
        libc::ENOSYS => "ENOSYS".into(),
        libc::ESRCH => "ESRCH".into(),
        n => format!("E{n}"),
    }
}

#[cfg(target_os = "linux")]
fn probes(exec: bool) -> String {
    // SAFETY: plain syscalls with valid (or null) pointers; results only inspected.
    unsafe {
        let fds: Vec<String> = (0..1024)
            .filter(|fd| libc::fcntl(*fd, libc::F_GETFD) >= 0)
            .map(|fd| fd.to_string())
            .collect();
        let mut out = vec![];
        let mut p = |k: &str, r: i64| out.push(format!("{k}={}", errno_name(r)));
        p("inet", libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) as i64);
        p("unix", libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) as i64);
        p(
            "passwd",
            libc::open(c"/etc/passwd".as_ptr(), libc::O_RDONLY) as i64,
        );
        p(
            "home",
            libc::open(c"/home".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) as i64,
        );
        p(
            "tmp",
            libc::open(
                c"/tmp/qrtz-speech-probe".as_ptr(),
                libc::O_CREAT | libc::O_WRONLY,
                0o600,
            ) as i64,
        );
        p("kill1", libc::kill(1, 0) as i64);
        p("killppid", libc::kill(libc::getppid(), 0) as i64);
        if exec {
            p(
                "ptrace",
                libc::ptrace(libc::PTRACE_ATTACH, libc::getppid(), 0, 0) as i64,
            );
        }
        let mut params = [0u8; 120];
        p("uring", libc::syscall(425, 1u32, params.as_mut_ptr()));
        p(
            "vmread",
            libc::syscall(
                libc::SYS_process_vm_readv,
                libc::getppid(),
                0usize,
                0usize,
                0usize,
                0usize,
                0usize,
            ),
        );
        p("prctl", libc::prctl(libc::PR_SET_DUMPABLE, 1, 0, 0, 0) as i64);

        // Last: if exec worked, this process is gone and the session crashes (a visible failure).
        let argv = [c"true".as_ptr(), std::ptr::null()];
        let envp = [std::ptr::null()];
        if exec {
            p(
                "exec",
                libc::execve(c"/usr/bin/true".as_ptr(), argv.as_ptr(), envp.as_ptr()) as i64,
            );
        }
        format!("probe {} fds={}", out.join(" "), fds.join(","))
    }
}

#[cfg(not(target_os = "linux"))]
fn probes(_exec: bool) -> String {
    "probe unsupported".into()
}

struct Out(File);
impl Out {
    fn send(&mut self, v: Value) {
        let _ = writeln!(self.0, "{v}");
        let _ = self.0.flush();
    }
}

fn set_nonblocking(fd: i32) {
    // SAFETY: fcntl on an inherited fd.
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
    }
}

struct Chan {
    name: &'static str,
    fd: i32,
    bytes: u64,
    utterances: u64,
    eof: bool,
    last_end: u64,
}

fn main() {
    // SAFETY: the host guarantees these fds (protocol v1 layout).
    let (control, results, model) =
        unsafe { (File::from_raw_fd(3), File::from_raw_fd(4), File::from_raw_fd(7)) };
    let mut cfg = String::new();
    let _ = (&model).read_to_string(&mut cfg);
    let cfg: Value = serde_json::from_str(&cfg).unwrap_or_else(|_| json!({}));
    let version = cfg["version"].as_str().unwrap_or("0.0.0").to_string();
    let mode = cfg["mode"].as_str().unwrap_or("normal").to_string();
    let say: Vec<String> = cfg["say"]
        .as_str()
        .unwrap_or("hello")
        .split_whitespace()
        .map(String::from)
        .collect();
    let mut out = Out(results);
    eprintln!("qrtz-mock-speech {version} mode={mode}");
    if mode == "silent" {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
    out.send(json!({"type": "hello", "protocol": 1, "moduleVersion": version, "languages": ["en"], "maxSpeakersPerChannel": 8}));
    if mode == "crash" {
        std::process::exit(3);
    }
    if mode == "probe" {
        out.send(json!({"type": "log", "level": "info", "msg": probes(cfg["probeUnsafe"] != false)}));
    }
    let mut ctl = BufReader::new(control);
    let session;
    let mut line = String::new();
    // Wait for start.
    loop {
        line.clear();
        if ctl.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        match v["type"].as_str() {
            Some("start") => {
                session = v["sessionId"].as_str().unwrap_or("").to_string();
                break;
            }
            Some("shutdown") => return,
            _ => {}
        }
    }
    out.send(json!({"type": "status", "phase": "ready"}));
    if mode == "garbage" {
        for _ in 0..3 {
            out.send(json!({"type": "words", "nope": true}));
        }
    }
    let mut chans = [
        Chan {
            name: "mic",
            fd: 5,
            bytes: 0,
            utterances: 0,
            eof: false,
            last_end: 0,
        },
        Chan {
            name: "system",
            fd: 6,
            bytes: 0,
            utterances: 0,
            eof: false,
            last_end: 0,
        },
    ];
    for c in &chans {
        set_nonblocking(c.fd);
    }
    set_nonblocking(3);
    let reads = mode != "never-reads";
    let mut next_word = 0usize;
    let mut buf = vec![0u8; 65536];
    // Lines the start reader already buffered (e.g. a `finish` sent right after `start`).
    let mut ctl_buf = ctl.buffer().to_vec();
    loop {
        let mut fds = vec![libc::pollfd {
            fd: 3,
            events: libc::POLLIN,
            revents: 0,
        }];
        if reads {
            for c in chans.iter().filter(|c| !c.eof) {
                fds.push(libc::pollfd {
                    fd: c.fd,
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
        }
        // SAFETY: fds is a valid pollfd array.
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 1000) };
        if reads {
            for c in chans.iter_mut().filter(|c| !c.eof) {
                // SAFETY: buf is valid for its length.
                let n = unsafe { libc::read(c.fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n == 0 {
                    c.eof = true;
                } else if n > 0 {
                    c.bytes += n as u64;
                    while c.bytes / (1000 * BYTES_PER_MS) > c.utterances {
                        c.utterances += 1;
                        let start = (c.utterances - 1) * 1000;
                        let w = &say[next_word % say.len()];
                        next_word += 1;
                        c.last_end = start + 800;
                        out.send(json!({"type": "words", "utteranceId": format!("{}-{}", c.name, c.utterances), "channel": c.name, "final": true, "language": "en", "words": [{"text": w, "startMs": start + 100, "endMs": start + 800}]}));
                    }
                }
            }
        }
        // Control: whole lines only.
        // SAFETY: buf is valid for its length.
        let n = unsafe { libc::read(3, buf.as_mut_ptr().cast(), buf.len()) };
        if n == 0 {
            return;
        }
        if n > 0 {
            ctl_buf.extend_from_slice(&buf[..n as usize]);
        }
        while let Some(i) = ctl_buf.iter().position(|b| *b == b'\n') {
            let l: Vec<u8> = ctl_buf.drain(..=i).collect();
            let v: Value = serde_json::from_slice(&l).unwrap_or(Value::Null);
            match v["type"].as_str() {
                Some("finish") => {
                    if reads {
                        // Drain what is already written.
                        for c in chans.iter_mut() {
                            loop {
                                // SAFETY: as above.
                                let n = unsafe { libc::read(c.fd, buf.as_mut_ptr().cast(), buf.len()) };
                                if n <= 0 {
                                    break;
                                }
                                c.bytes += n as u64;
                            }
                        }
                    }
                    for c in chans.iter_mut() {
                        if c.bytes > 0 && c.utterances == 0 {
                            let ms = (c.bytes / BYTES_PER_MS).max(1);
                            let step = (ms / say.len() as u64).max(1);
                            let words: Vec<Value> = say
                                .iter()
                                .enumerate()
                                .map(|(i, w)| {
                                    let s = (i as u64 * step).min(ms);
                                    json!({"text": w, "startMs": s, "endMs": (s + step).min(ms)})
                                })
                                .collect();
                            out.send(json!({"type": "words", "utteranceId": format!("{}-final", c.name), "channel": c.name, "final": true, "language": "en", "words": words}));
                        }
                    }
                    let dur = chans.iter().map(|c| c.bytes / BYTES_PER_MS).max().unwrap_or(0);
                    out.send(json!({"type": "finished", "sessionId": session, "durationMs": dur}));
                }
                Some("cancel") | Some("shutdown") => return,
                _ => {}
            }
        }
    }
}

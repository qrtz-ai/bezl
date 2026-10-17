//! Confinement of the speech module process (ADR 0046 §6, review N58/N59/N76).
//!
//! Linux: the child gets exactly fds 0..7+n (everything else is close-on-exec), `PR_SET_NO_NEW_PRIVS`, a Landlock
//! ruleset (no file access except reading the loader and libraries and executing the verified entry and the
//! loader; no TCP; abstract unix sockets and signals scoped at ABI >= 6), and a seccomp **allowlist** (no
//! `socket`, `connect`, `bind`, `io_uring_*`, `ptrace`, `process_vm_*`, `open_by_handle_at`, `prctl`; `kill` /
//! `tgkill` only to its own pid). The entry is exec'd from its verified, hashed fd (`execveat(AT_EMPTY_PATH)`),
//! never by path. Without Landlock or seccomp, `Confinement::probe` says so and release modules are refused.
//!
//! Everything is built in the parent; the child (between fork and exec) only dups fds, makes syscalls and patches
//! one integer in the prebuilt filter: no allocation.
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(not(target_os = "linux"))]
pub use other::*;

#[cfg(not(target_os = "linux"))]
mod other {
    /// macOS: posix_spawn with POSIX_SPAWN_CLOEXEC_DEFAULT + the App Sandbox profile + code requirement and
    /// `sandbox_check` (N57) are Mac-only work; until then the host refuses to spawn modules here.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Confinement {
        pub landlock_abi: i32,
        pub seccomp: bool,
    }
    impl Confinement {
        pub fn probe() -> Self {
            Confinement {
                landlock_abi: 0,
                seccomp: false,
            }
        }
        pub fn available(&self) -> bool {
            false
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, OwnedFd, RawFd};
    use std::path::{Path, PathBuf};

    /// What the kernel offers.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Confinement {
        /// 0 = no Landlock.
        pub landlock_abi: i32,
        pub seccomp: bool,
    }

    const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
    const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
    const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;
    const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
    const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;

    const FS_EXECUTE: u64 = 1 << 0;
    const FS_READ_FILE: u64 = 1 << 2;
    const FS_READ_DIR: u64 = 1 << 3;

    fn fs_all(abi: i32) -> u64 {
        let mut v = (1u64 << 13) - 1; // ABI 1: EXECUTE..MAKE_SYM
        if abi >= 2 {
            v |= 1 << 13; // REFER
        }
        if abi >= 3 {
            v |= 1 << 14; // TRUNCATE
        }
        if abi >= 5 {
            v |= 1 << 15; // IOCTL_DEV
        }
        v
    }

    #[repr(C)]
    struct RulesetAttr {
        fs: u64,
        net: u64,
        scoped: u64,
    }

    #[repr(C, packed)]
    struct PathBeneath {
        allowed: u64,
        parent_fd: i32,
    }

    impl Confinement {
        pub fn probe() -> Self {
            // SAFETY: the version query takes no pointers.
            let abi = unsafe {
                libc::syscall(
                    SYS_LANDLOCK_CREATE_RULESET,
                    std::ptr::null::<u8>(),
                    0usize,
                    LANDLOCK_CREATE_RULESET_VERSION,
                )
            };
            // SAFETY: PR_GET_SECCOMP has no pointer arguments.
            let seccomp = unsafe { libc::prctl(libc::PR_GET_SECCOMP) } >= 0;
            Confinement {
                landlock_abi: abi.max(0) as i32,
                seccomp,
            }
        }
        pub fn available(&self) -> bool {
            self.landlock_abi >= 1 && self.seccomp
        }
    }

    fn os_err(what: &str) -> std::io::Error {
        let e = std::io::Error::last_os_error();
        std::io::Error::new(e.kind(), format!("{what}: {e}"))
    }

    fn open_path(p: &Path) -> std::io::Result<OwnedFd> {
        let c = CString::new(p.as_os_str().as_encoded_bytes()).map_err(std::io::Error::other)?;
        // SAFETY: c is a valid C string; the fd is owned below.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(os_err(&format!("open {}", p.display())));
        }
        // SAFETY: fresh fd we own.
        Ok(unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) })
    }

    /// The program interpreter (`PT_INTERP`) of an ELF, if dynamic.
    fn elf_interp(bytes: &[u8]) -> Option<PathBuf> {
        if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" || bytes[4] != 2 {
            return None;
        }
        let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]) as usize;
        let u64_at = |o: usize| u64::from_le_bytes(bytes[o..o + 8].try_into().unwrap()) as usize;
        let (phoff, phentsize, phnum) = (u64_at(32), u16_at(54), u16_at(56));
        for i in 0..phnum {
            let ph = phoff + i * phentsize;
            if ph + 56 > bytes.len() {
                return None;
            }
            if u32::from_le_bytes(bytes[ph..ph + 4].try_into().unwrap()) == 3 {
                let (off, sz) = (u64_at(ph + 8), u64_at(ph + 32));
                let s = bytes.get(off..off + sz)?;
                let s = s.split(|b| *b == 0).next()?;
                return Some(PathBuf::from(std::str::from_utf8(s).ok()?));
            }
        }
        None
    }

    /// A Landlock ruleset for one module: read the loader and system libraries, execute the entry and the loader,
    /// nothing else; no TCP; scoped signals and abstract unix sockets at ABI >= 6.
    /// `entry` is the verified entry fd itself (a rule may name any fd of the file), so no path is reopened.
    pub fn landlock_ruleset(abi: i32, entry: RawFd, entry_head: &[u8]) -> std::io::Result<OwnedFd> {
        let attr = RulesetAttr {
            fs: fs_all(abi),
            net: if abi >= 4 { 0b11 } else { 0 },
            scoped: if abi >= 6 { 0b11 } else { 0 },
        };
        let size = match abi {
            1..=3 => 8,
            4 | 5 => 16,
            _ => 24,
        };
        // SAFETY: attr outlives the call; size matches the ABI.
        let fd = unsafe {
            libc::syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                &attr as *const RulesetAttr,
                size as usize,
                0u32,
            )
        };
        if fd < 0 {
            return Err(os_err("landlock_create_ruleset"));
        }
        // SAFETY: fresh fd we own.
        let rs: OwnedFd = unsafe { std::os::fd::FromRawFd::from_raw_fd(fd as RawFd) };
        let add_fd = |fd: RawFd, allowed: u64, what: &str| -> std::io::Result<()> {
            let rule = PathBeneath {
                allowed,
                parent_fd: fd,
            };
            // SAFETY: rule outlives the call.
            let r = unsafe {
                libc::syscall(
                    SYS_LANDLOCK_ADD_RULE,
                    rs.as_raw_fd(),
                    LANDLOCK_RULE_PATH_BENEATH,
                    &rule as *const PathBeneath,
                    0u32,
                )
            };
            if r < 0 {
                return Err(os_err(&format!("landlock_add_rule {what}")));
            }
            Ok(())
        };
        let add = |p: &Path, allowed: u64| -> std::io::Result<()> {
            let target = open_path(p)?;
            add_fd(target.as_raw_fd(), allowed, &p.display().to_string())
        };
        add_fd(entry, FS_EXECUTE | FS_READ_FILE, "entry")?;
        for d in ["/lib", "/lib64", "/usr/lib", "/usr/lib64"] {
            let p = Path::new(d);
            if p.is_dir() {
                add(p, FS_READ_FILE | FS_READ_DIR)?;
            }
        }
        if Path::new("/etc/ld.so.cache").is_file() {
            add(Path::new("/etc/ld.so.cache"), FS_READ_FILE)?;
        }
        if let Some(interp) = elf_interp(entry_head) {
            let real = std::fs::canonicalize(&interp)?;
            add(&real, FS_EXECUTE | FS_READ_FILE)?;
        }
        Ok(rs)
    }

    // ---- seccomp ----

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct SockFilter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }
    #[repr(C)]
    struct SockFprog {
        len: u16,
        filter: *const SockFilter,
    }

    const LD_W_ABS: u16 = 0x20;
    const JEQ_K: u16 = 0x15;
    const RET_K: u16 = 0x06;
    const RET_ALLOW: u32 = 0x7fff_0000;
    const RET_KILL_PROCESS: u32 = 0x8000_0000;
    const RET_EPERM: u32 = 0x0005_0000 | libc::EPERM as u32;
    #[cfg(target_arch = "x86_64")]
    const AUDIT_ARCH: u32 = 0xC000_003E;
    #[cfg(target_arch = "aarch64")]
    const AUDIT_ARCH: u32 = 0xC000_00B7;

    fn st(code: u16, jt: u8, jf: u8, k: u32) -> SockFilter {
        SockFilter { code, jt, jf, k }
    }

    /// Syscalls a module may make. Anything else fails with EPERM (so probes see it); a foreign arch is killed.
    fn allowed() -> Vec<libc::c_long> {
        use libc::*;
        #[allow(unused_mut)]
        let mut v = vec![
            SYS_read,
            SYS_write,
            SYS_readv,
            SYS_writev,
            SYS_pread64,
            SYS_close,
            SYS_fstat,
            SYS_newfstatat,
            SYS_statx,
            SYS_lseek,
            SYS_mmap,
            SYS_munmap,
            SYS_mprotect,
            SYS_mremap,
            SYS_madvise,
            SYS_brk,
            SYS_rt_sigaction,
            SYS_rt_sigprocmask,
            SYS_rt_sigreturn,
            SYS_sigaltstack,
            SYS_futex,
            SYS_clone,
            SYS_clone3,
            SYS_set_robust_list,
            SYS_rseq,
            SYS_set_tid_address,
            SYS_gettid,
            SYS_getpid,
            SYS_getppid,
            SYS_exit,
            SYS_exit_group,
            SYS_sched_yield,
            SYS_sched_getaffinity,
            SYS_nanosleep,
            SYS_clock_nanosleep,
            SYS_clock_gettime,
            SYS_getrandom,
            SYS_ppoll,
            SYS_openat,
            SYS_faccessat,
            SYS_faccessat2,
            SYS_readlinkat,
            SYS_fcntl,
            SYS_dup,
            SYS_dup3,
            SYS_getuid,
            SYS_geteuid,
            SYS_getgid,
            SYS_getegid,
            SYS_uname,
            SYS_prlimit64,
            SYS_execve,
            SYS_execveat,
            SYS_getcwd,
            SYS_membarrier,
        ];
        #[cfg(target_arch = "x86_64")]
        v.extend([
            SYS_arch_prctl,
            SYS_access,
            SYS_open,
            SYS_stat,
            SYS_lstat,
            SYS_poll,
            SYS_readlink,
            SYS_dup2,
        ]);
        v
    }

    /// The prebuilt filter; [`SeccompFilter::patch_pid`] is the only change made in the child.
    pub struct SeccompFilter {
        prog: Vec<SockFilter>,
        pid_slots: Vec<usize>,
    }

    impl SeccompFilter {
        pub fn build() -> Self {
            let mut p = vec![
                st(LD_W_ABS, 0, 0, 4), // arch
                st(JEQ_K, 1, 0, AUDIT_ARCH),
                st(RET_K, 0, 0, RET_KILL_PROCESS),
                st(LD_W_ABS, 0, 0, 0), // nr
            ];
            let mut pid_slots = vec![];
            for sys in [libc::SYS_kill, libc::SYS_tgkill] {
                p.push(st(JEQ_K, 0, 4, sys as u32));
                p.push(st(LD_W_ABS, 0, 0, 16)); // args[0], low 32 bits
                pid_slots.push(p.len());
                p.push(st(JEQ_K, 0, 1, 0)); // patched: own pid
                p.push(st(RET_K, 0, 0, RET_ALLOW));
                p.push(st(RET_K, 0, 0, RET_EPERM));
            }
            let list = allowed();
            let n = list.len();
            for (i, sys) in list.into_iter().enumerate() {
                // Match: jump over the remaining compares and the EPERM return to ALLOW.
                p.push(st(JEQ_K, (n - 1 - i + 1) as u8, 0, sys as u32));
            }
            p.push(st(RET_K, 0, 0, RET_EPERM));
            p.push(st(RET_K, 0, 0, RET_ALLOW));
            SeccompFilter { prog: p, pid_slots }
        }

        /// Child only: no allocation.
        fn patch_pid(&mut self, pid: u32) {
            for i in 0..self.pid_slots.len() {
                let s = self.pid_slots[i];
                self.prog[s].k = pid;
            }
        }
    }

    /// Everything the child needs, prepared in the parent.
    pub struct ChildPlan {
        /// `targets[i]` becomes fd `i` in the child (0..7+n).
        pub targets: Vec<RawFd>,
        pub entry: RawFd,
        pub ruleset: Option<OwnedFd>,
        pub seccomp: Option<SeccompFilter>,
        pub argv: Vec<CString>,
        argv_ptrs: Vec<*const libc::c_char>,
        envp_ptrs: Vec<*const libc::c_char>,
        scratch: Vec<RawFd>,
    }

    // The raw pointers point into `argv`, owned by the plan; the plan moves into the pre_exec closure whole.
    unsafe impl Send for ChildPlan {}
    unsafe impl Sync for ChildPlan {}

    impl ChildPlan {
        pub fn new(
            targets: Vec<RawFd>,
            entry: RawFd,
            ruleset: Option<OwnedFd>,
            seccomp: Option<SeccompFilter>,
            argv: Vec<CString>,
        ) -> Self {
            let mut argv_ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
            argv_ptrs.push(std::ptr::null());
            let n = targets.len();
            ChildPlan {
                targets,
                entry,
                ruleset,
                seccomp,
                argv,
                argv_ptrs,
                envp_ptrs: vec![std::ptr::null()],
                scratch: vec![-1; n],
            }
        }

        /// Runs in the child between fork and exec. On success it does not return (the module image replaces it).
        ///
        /// # Safety
        /// Only async-signal-safe calls; called from `CommandExt::pre_exec`.
        pub unsafe fn exec_in_child(&mut self) -> std::io::Result<()> {
            let n = self.targets.len() as RawFd;
            // 1. Move every source above the target range (so no dup2 clobbers a later source).
            for i in 0..self.targets.len() {
                let fd = libc::fcntl(self.targets[i], libc::F_DUPFD_CLOEXEC, n + 16);
                if fd < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                self.scratch[i] = fd;
            }
            let entry = libc::fcntl(self.entry, libc::F_DUPFD_CLOEXEC, n + 16);
            if entry < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // 2. Place them (dup2 clears close-on-exec on the target).
            for i in 0..self.scratch.len() {
                if libc::dup2(self.scratch[i], i as RawFd) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            // 3. N58: everything else is close-on-exec (incl. the scratch copies, the entry and std's error pipe).
            if libc::syscall(
                libc::SYS_close_range,
                n as u32,
                u32::MAX,
                4u32, /* CLOSE_RANGE_CLOEXEC */
            ) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if let Some(rs) = &self.ruleset {
                if libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, rs.as_raw_fd(), 0u32) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if let Some(f) = &mut self.seccomp {
                f.patch_pid(libc::getpid() as u32);
                let prog = SockFprog {
                    len: f.prog.len() as u16,
                    filter: f.prog.as_ptr(),
                };
                if libc::prctl(
                    libc::PR_SET_SECCOMP,
                    2, /* FILTER */
                    &prog as *const SockFprog,
                    0,
                    0,
                ) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
            let empty = c"".as_ptr();
            libc::syscall(
                libc::SYS_execveat,
                entry,
                empty,
                self.argv_ptrs.as_ptr(),
                self.envp_ptrs.as_ptr(),
                libc::AT_EMPTY_PATH,
            );
            Err(std::io::Error::last_os_error())
        }
    }
}

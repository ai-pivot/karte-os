//! Linux syscall compatibility layer.
//!
//! Supports both RISC-V and x86_64 Linux syscall numbers.
//! Go binaries on x86_64 use the `syscall` instruction (0x0F 0x05) with
//! x86_64 Linux syscall numbers, which differ completely from RISC-V.
//!
//! ## Design
//!
//! - **Runtime opt-in**: controlled by `ENABLED` atomic bool
//! - **Architecture-aware**: `translate()` handles both RISC-V and x86_64 syscall numbers
//! - **Zero intrusion**: existing KarteOS syscall handlers are unchanged
//! - **Argument adaptation**: some Linux syscalls have different argument layouts

use core::sync::atomic::{AtomicBool, Ordering};

/// Global runtime switch for the Linux compatibility layer.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable the Linux compatibility layer.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Disable the Linux compatibility layer.
pub fn disable() {
    ENABLED.store(false, Ordering::Relaxed);
}

/// Check whether the Linux compatibility layer is enabled.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

// ─── Linux x86_64 syscall number constants ─────────────────────
#[cfg(target_arch = "x86_64")]
mod x86_64_syscalls {
    pub const L_READ: usize = 0;
    pub const L_WRITE: usize = 1;
    pub const L_OPEN: usize = 2;
    pub const L_CLOSE: usize = 3;
    pub const L_STAT: usize = 4;
    pub const L_FSTAT: usize = 5; // Linux x86_64 fstat = 5
    pub const L_POLL: usize = 7;
    pub const L_LSEEK: usize = 8; // Linux x86_64 lseek = 8
    pub const L_MMAP: usize = 9;
    pub const L_MPROTECT: usize = 10;
    pub const L_MUNMAP: usize = 11;
    pub const L_BRK: usize = 12;
    pub const L_RT_SIGACTION: usize = 13;
    pub const L_RT_SIGPROCMASK: usize = 14;
    pub const L_IOCTL: usize = 16;
    pub const L_PREAD64: usize = 17;
    pub const L_ACCESS: usize = 21;
    pub const L_PIPE: usize = 22;
    pub const L_SELECT: usize = 23;
    pub const L_SCHED_YIELD: usize = 24;
    pub const L_MADVISE: usize = 28;
    pub const L_DUP: usize = 32;
    pub const L_NANOSLEEP: usize = 35;
    pub const L_GETPID: usize = 39;
    pub const L_SOCKET: usize = 41;
    pub const L_CONNECT: usize = 42;
    pub const L_CLONE: usize = 56;
    pub const L_FORK: usize = 57;
    pub const L_EXECVE: usize = 59;
    pub const L_EXIT: usize = 60;
    pub const L_EXIT_GROUP: usize = 231;
    pub const L_WAIT4: usize = 61;
    pub const L_KILL: usize = 62;
    pub const L_FCNTL: usize = 72;
    pub const L_FSYNC: usize = 74;
    pub const L_GETDENTS: usize = 78;
    pub const L_GETDENTS64: usize = 217;
    pub const L_GETCWD: usize = 79;
    pub const L_CHDIR: usize = 80;
    pub const L_MKDIR: usize = 83;
    pub const L_RMDIR: usize = 84;
    pub const L_UNLINK: usize = 87;
    pub const L_READLINK: usize = 89;
    pub const L_GETUID: usize = 102;
    pub const L_GETGID: usize = 104;
    pub const L_GETEUID: usize = 107;
    pub const L_GETEGID: usize = 108;
    pub const L_SIGALTSTACK: usize = 131;
    pub const L_GETTIMEOFDAY: usize = 96;
    pub const L_GETRLIMIT: usize = 97;
    pub const L_GETRUSAGE: usize = 98;
    pub const L_SYSINFO: usize = 99;
    pub const L_TIMES: usize = 100;
    pub const L_PTRACE: usize = 101;
    pub const L_SET_TID_ADDR: usize = 218;
    pub const L_SETRLIMIT: usize = 160;
    pub const L_GETPID2: usize = 39;
    pub const L_GETTID: usize = 186;
    pub const L_UNAME: usize = 122;
    pub const L_TGKILL: usize = 234;
    pub const L_TKILL: usize = 200;
    pub const L_TIME: usize = 201;
    pub const L_FUTEX: usize = 202;
    pub const L_SCHED_SETAFFINITY: usize = 203;
    pub const L_SCHED_GETAFFINITY: usize = 204;
    pub const L_SET_THREAD_AREA: usize = 205;
    pub const L_GET_THREAD_AREA: usize = 211;
    pub const L_CLOCK_GETTIME: usize = 228;
    pub const L_CLOCK_GETRES: usize = 229;
    pub const L_OPENAT: usize = 257;
    pub const L_MKDIRAT: usize = 258;
    pub const L_NEWFSTATAT: usize = 262;
    pub const L_UNLINKAT: usize = 263;
    pub const L_READLINKAT: usize = 267;
    pub const L_FACCESSAT: usize = 269;
    pub const L_PRLIMIT64: usize = 302;
    pub const L_GETRANDOM: usize = 318;
    pub const L_ARCH_PRCTL: usize = 158;
    pub const L_SYSLOG: usize = 103;
    pub const L_PRCTL: usize = 157;
    pub const L_SET_ROBUST_LIST: usize = 273;
    pub const L_GET_ROBUST_LIST: usize = 274;
    pub const L_RSEQ: usize = 334;
    pub const L_UNSHARE: usize = 272;
    pub const L_EPOLL_CREATE1: usize = 291;
    pub const L_EPOLL_CTL: usize = 233;
    pub const L_EPOLL_PWAIT: usize = 281;
    pub const L_EPOLL_WAIT: usize = 232;
    pub const L_EVENTFD2: usize = 290;
    pub const L_FACCESSAT2: usize = 439;
    pub const L_PIPE2: usize = 293;
    pub const L_DUP3: usize = 292;
    pub const L_MREMAP: usize = 25;
    pub const L_MINCORE: usize = 27;
    pub const L_SHMGET: usize = 29;
    pub const L_SHMAT: usize = 30;
    pub const L_SHMCTL: usize = 31;
    pub const L_RECVMSG: usize = 47;
    pub const L_SENDMSG: usize = 46;
    pub const L_LISTEN: usize = 50;
    pub const L_BIND: usize = 49;
    pub const L_ACCEPT: usize = 43;
    pub const L_SENDTO: usize = 44;
    pub const L_RECVFROM: usize = 45;
    pub const L_SHUTDOWN: usize = 48;
    pub const L_GETSOCKNAME: usize = 51;
    pub const L_GETPEERNAME: usize = 52;
    pub const L_SETSOCKOPT: usize = 54;
    pub const L_GETSOCKOPT: usize = 55;
    pub const L_SIGACTION: usize = 13;
    pub const L_RT_SIGRETURN: usize = 15;
}

// ─── Linux RISC-V syscall number constants ─────────────────────
#[cfg(target_arch = "riscv64")]
mod riscv_syscalls {
    pub const L_IOCTL: usize = 29;
    pub const L_OPENAT: usize = 56;
    pub const L_CLOSE: usize = 57;
    pub const L_LSEEK: usize = 62;
    pub const L_READ: usize = 63;
    pub const L_WRITE: usize = 64;
    pub const L_FSTAT: usize = 80;
    pub const L_EXIT: usize = 93;
    pub const L_EXIT_GROUP: usize = 94;
    pub const L_SET_TID_ADDR: usize = 96;
    pub const L_GETPID: usize = 172;
    pub const L_BRK: usize = 214;
    pub const L_MUNMAP: usize = 215;
    pub const L_MMAP: usize = 222;
    pub const L_MPROTECT: usize = 226;
    pub const L_MADVISE: usize = 233;
    pub const L_SOCKET: usize = 198;
    pub const L_BIND: usize = 200;
    pub const L_CONNECT: usize = 203;
    pub const L_LISTEN: usize = 201;
    pub const L_ACCEPT: usize = 202;
    pub const L_SENDTO: usize = 206;
    pub const L_RECVFROM: usize = 207;
    pub const L_SHUTDOWN: usize = 210;
    pub const L_SCHED_YIELD: usize = 124;
    pub const L_SCHED_GETAFFINITY: usize = 123;
    pub const L_FUTEX: usize = 98;
    pub const L_NANOSLEEP: usize = 101;
    pub const L_CLOCK_GETTIME: usize = 113;
    pub const L_CLOCK_GETRES: usize = 114;
    pub const L_UNAME: usize = 160;
    pub const L_GETCWD: usize = 17;
    pub const L_CHDIR: usize = 49;
    pub const L_GETTID: usize = 178;
    pub const L_KILL: usize = 129;
    pub const L_RT_SIGACTION: usize = 134;
    pub const L_RT_SIGPROCMASK: usize = 135;
    pub const L_RT_SIGRETURN: usize = 139;
    pub const L_SIGALTSTACK: usize = 132;
    pub const L_GETTIMEOFDAY: usize = 169;
    pub const L_SYSINFO: usize = 179;
    pub const L_PRCTL: usize = 167;
    pub const L_PRLIMIT64: usize = 261;
    pub const L_GETRANDOM: usize = 278;
    pub const L_EPOLL_CREATE1: usize = 20;
    pub const L_EPOLL_CTL: usize = 21;
    pub const L_EPOLL_PWAIT: usize = 22;
    pub const L_DUP: usize = 23;
    pub const L_DUP3: usize = 24;
    pub const L_FCNTL: usize = 25;
    pub const L_FACCESSAT: usize = 48;
    pub const L_PIPE2: usize = 59;
    pub const L_GETDENTS64: usize = 61;
    pub const L_READLINKAT: usize = 78;
    pub const L_NEWFSTATAT: usize = 79;
    pub const L_EVENTFD2: usize = 19;
    pub const L_TIMERFD_CREATE: usize = 85;
    pub const L_TIMERFD_SETTIME: usize = 86;
    pub const L_CLONE: usize = 220;
    pub const L_MKDIRAT: usize = 258;
    pub const L_MKDIRAT_OLD: usize = 34;
    pub const L_FTRUNCATE: usize = 46;
    pub const L_PREAD64: usize = 67;
    pub const L_PWRITE64: usize = 68;
    pub const L_READV: usize = 65;
    pub const L_WRITEV: usize = 66;
    pub const L_PREADV: usize = 69;
    pub const L_PWRITEV: usize = 70;
    pub const L_FSYNC: usize = 82;
    pub const L_FALLOCATE: usize = 47;
    pub const L_UNLINKAT: usize = 35;
    pub const L_GETEUID: usize = 175;
    pub const L_GETUID: usize = 174;
    pub const L_GETGID: usize = 176;
    pub const L_GETEGID: usize = 177;
}

// ─── Public API ──────────────────────────────────────────────────

/// Result of a successful syscall translation.
pub enum Translation {
    /// Translate to a KarteOS syscall number and (possibly adapted) args.
    Dispatch { karte_nr: usize, args: [usize; 6] },
    /// Already handled (stub or special case). Contains the return value.
    Handled(isize),
}

/// Try to translate a Linux syscall to a KarteOS equivalent.
/// Returns `None` if the syscall number is not a known Linux syscall
/// (i.e., it's already a KarteOS syscall number).
pub fn translate(id: usize, args: [usize; 6]) -> Option<Translation> {
    if !is_enabled() {
        return None;
    }

    #[cfg(target_arch = "x86_64")]
    {
        translate_x86_64(id, args)
    }
    #[cfg(target_arch = "riscv64")]
    {
        translate_riscv(id, args).or_else(|| translate_riscv_go(id, args))
    }
}

// ─── x86_64 Linux translation ────────────────────────────────────

#[cfg(target_arch = "x86_64")]
fn translate_x86_64(id: usize, args: [usize; 6]) -> Option<Translation> {
    use x86_64_syscalls::*;

    // ═══════════════════════════════════════════════════════════
    // Guard: protect KarteOS native syscall numbers from being
    // mis-translated. Both KarteOS programs and Go programs enter
    // via int 0x80, but they use different syscall number schemes.
    // KarteOS native range: 0-80 (with gaps).
    // Linux x86_64 range: 0-450+.
    // Numbers in the KarteOS range are NEVER translated — they go
    // directly to the native KarteOS dispatch in mod.rs.
    // ═══════════════════════════════════════════════════════════
    const KARTEOS_NATIVE_NUMBERS: &[usize] = &[
        // 0-8: debug_print, exit, write, read, brk, getpid, mmap, pipe, dup2
        0, 1, 2, 3, 4, 5, 6, 7, 8, // 10-11: open, close
        10, 11, // 30-34: spawn, waitpid, exec, exec_fd, fork
        30, 31, 32, 33, 34, // 40-42: ls, mkdir, unlink
        40, 41, 42, // 50-52: setenv, getenv, chdir
        50, 51, 52, // 60-61: kill, sigret
        60, 61, // 70-77: socket..shutdown
        70, 71, 72, 73, 74, 75, 76, 77,
    ];
    if KARTEOS_NATIVE_NUMBERS.contains(&id) {
        return None;
    }

    // Also skip L_BRK(12): Linux brk is functionally equivalent to
    // KarteOS SYS_BRK(4). Since Go binaries use int 0x80 + Linux
    // numbers, a Go brk(12) should not be intercepted here — let
    // it fall through to the `_ => None` and be handled by the
    // caller (or the Go program should use the Linux mmap path).
    if id == L_BRK {
        return None;
    }

    match id {
        // ─── Memory management ───────────────────────────────
        // Linux mmap(addr, len, prot, flags, fd, offset) → LINUX_MMAP (dedicated handler)
        L_MMAP => Some(Translation::Dispatch {
            karte_nr: 110, // LINUX_MMAP
            args,
        }),
        L_MPROTECT => Some(Translation::Dispatch {
            karte_nr: 111, // LINUX_MPROTECT
            args: [args[0], args[1], args[2], 0, 0, 0],
        }),
        L_MUNMAP => Some(Translation::Dispatch {
            karte_nr: 112, // LINUX_MUNMAP
            args: [args[0], args[1], 0, 0, 0, 0],
        }),
        L_MADVISE => Some(Translation::Dispatch {
            karte_nr: 114, // LINUX_MADVISE
            args: [args[0], args[1], args[2], 0, 0, 0],
        }),

        // ─── Process management ──────────────────────────────
        L_EXIT_GROUP => Some(Translation::Dispatch {
            karte_nr: super::SYS_EXIT,
            args,
        }),
        L_GETPID => Some(Translation::Dispatch {
            karte_nr: super::SYS_GETPID,
            args,
        }),
        L_GETTID => Some(Translation::Dispatch {
            karte_nr: super::SYS_GETPID,
            args,
        }),

        // ─── clone/fork ──────────────────────────────────────
        // Linux clone(flags, stack, parent_tid, tls, child_tid)
        // TEMPORARILY DISABLED: return -ENOSYS to force single-threaded mode
        L_CLONE => Some(Translation::Handled(-38)), // -ENOSYS
        L_FORK => Some(Translation::Dispatch {
            karte_nr: super::SYS_FORK,
            args,
        }),

        // ─── futex ───────────────────────────────────────────
        // Linux futex(addr, op, val, timeout, uaddr2, val3)
        // We delegate to a dedicated handler
        L_FUTEX => Some(Translation::Dispatch {
            karte_nr: 101, // LINUX_FUTEX syscall number
            args,
        }),

        // Signal handlers are in translate_riscv_go with correct RISC-V numbers

        // ─── File system (Linux-only numbers, no KarteOS conflict) ──
        // Note: L_OPEN(2), L_CLOSE(3), L_READ(0), L_WRITE(1) are
        // blocked by the guard above since they collide with KarteOS.
        // Linux programs should use L_OPENAT(257) and L_EXIT_GROUP(231).
        L_OPENAT => {
            // Linux openat(dirfd, pathname, flags, mode)
            let path_ptr = args[1];
            let flags = args[2];
            let path_len = count_user_string(path_ptr);
            if path_len == 0 {
                return Some(Translation::Handled(super::ERR_NOENT));
            }
            Some(Translation::Dispatch {
                karte_nr: super::SYS_OPEN,
                args: [path_ptr, path_len, flags, 0, 0, 0],
            })
        }
        L_NEWFSTATAT => {
            let pl = count_user_string(args[1]);
            if pl == 0 {
                return Some(Translation::Handled(super::ERR_NOENT));
            }
            let name = match super::read_user_path(args[1], pl) {
                Some(n) => n,
                None => return Some(Translation::Handled(super::ERR_NOENT)),
            };
            let name = super::resolve_path(&name);
            if let Some(inode) = crate::driver::fs::lookup_path(&name) {
                if args[2] != 0 {
                    let meta = crate::driver::ext4::metadata_of(inode);
                    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let fsize = meta.as_ref().map(|m| m.size as u64).unwrap_or(0);
                    let mode = if is_dir { 0x41EDu32 } else { 0x81A4u32 };
                    for i in 0..128usize {
                        super::user_write_u8(args[2] + i, 0);
                    }
                    super::user_write::<u32>(args[2] + 16, mode);
                    super::user_write::<u32>(args[2] + 20, 1);
                    super::user_write::<u64>(args[2] + 48, fsize);
                    super::user_write::<u32>(args[2] + 56, 4096);
                }
                Some(Translation::Handled(0))
            } else {
                Some(Translation::Handled(super::ERR_NOENT))
            }
        } // stub
        L_FSTAT => {
            // fstat(fd, stat_ptr) — fill x86_64 struct stat (144 bytes)
            let result = sys_fstat(args[0] as i32, args[1]);
            Some(Translation::Handled(result))
        }
        // Note: L_STAT(4) removed — conflicts with KarteOS SYS_BRK(4)
        L_PREAD64 => Some(Translation::Dispatch {
            karte_nr: super::SYS_READ,
            args: [args[0], args[1], args[2], 0, 0, 0], // fd, buf, count
        }),
        // Note: L_DUP(32) removed — conflicts with KarteOS SYS_EXEC(32)
        L_PIPE | L_PIPE2 => Some(Translation::Dispatch {
            karte_nr: super::SYS_PIPE,
            args,
        }),
        // getdents handled by translate_riscv_go
        L_GETCWD => {
            let result = sys_getcwd(args[0], args[1]);
            Some(Translation::Handled(result))
        }
        // Note: L_CHDIR(80) conflicts with KarteOS SYS_IOCTL(80) — use chdir via openat
        L_KILL => Some(Translation::Handled(0)), // stub: Go doesn't use kill critically
        L_MKDIR => Some(Translation::Dispatch {
            karte_nr: super::SYS_MKDIR,
            args: [args[0], count_user_string(args[0]), 0, 0, 0, 0],
        }),
        L_UNLINK => Some(Translation::Dispatch {
            karte_nr: super::SYS_UNLINK,
            args: [args[0], count_user_string(args[0]), 0, 0, 0, 0],
        }),
        L_READLINK | L_READLINKAT => Some(Translation::Handled(super::ERR_NOENT)), // ENOENT
        L_ACCESS | L_FACCESSAT | L_FACCESSAT2 => Some(Translation::Handled(0)),    // stub
        L_IOCTL => Some(Translation::Dispatch {
            karte_nr: super::SYS_IOCTL,
            args,
        }), // stub

        // ─── Scheduling ──────────────────────────────────────
        L_SCHED_YIELD => Some(Translation::Dispatch {
            karte_nr: 106, // LINUX_SCHED_YIELD
            args,
        }),
        L_SCHED_GETAFFINITY => {
            // sched_getaffinity(pid, size, mask) — fake: CPU 0 available
            let size = args[1]; // mask_size in bytes
            let mask_ptr = args[2];
            if mask_ptr != 0 && size > 0 {
                for i in 0..size {
                    crate::syscall::user_write_u8(mask_ptr + i, 0);
                }
                crate::syscall::user_write_u8(mask_ptr, 0x01);
            }
            Some(Translation::Handled(size as isize))
        }

        // ─── Time ────────────────────────────────────────────
        L_GETTIMEOFDAY => {
            // gettimeofday(tv, tz) — return fake time based on uptime
            let uptime_ms = crate::arch::platform::uptime_ms();
            let tv_sec = (uptime_ms / 1000 + super::FAKE_EPOCH) as i64;
            let tv_usec = ((uptime_ms % 1000) * 1000) as i64;
            if args[0] != 0 {
                let tv_ptr = args[0];
                crate::syscall::user_write::<i64>(tv_ptr, tv_sec);
                crate::syscall::user_write::<i64>(tv_ptr + 8, tv_usec);
            }
            // tz is ignored (obsolete)
            Some(Translation::Handled(0))
        }
        L_CLOCK_GETTIME => {
            const CLOCK_REALTIME: usize = 0;
            const CLOCK_MONOTONIC: usize = 1;
            const CLOCK_PROCESS_CPUTIME_ID: usize = 2;
            const CLOCK_THREAD_CPUTIME_ID: usize = 3;
            const CLOCK_MONOTONIC_RAW: usize = 4;
            const CLOCK_REALTIME_COARSE: usize = 5;
            const CLOCK_MONOTONIC_COARSE: usize = 6;
            const CLOCK_BOOTTIME: usize = 7;

            if args[1] != 0 {
                let clockid = args[0];
                let tp = args[1];
                let uptime_ms = crate::arch::platform::uptime_ms();
                let (secs, nsecs) = match clockid {
                    CLOCK_REALTIME | CLOCK_REALTIME_COARSE => (
                        (super::FAKE_EPOCH + uptime_ms / 1000) as i64,
                        ((uptime_ms % 1000) * 1_000_000) as i64,
                    ),
                    CLOCK_MONOTONIC
                    | CLOCK_MONOTONIC_RAW
                    | CLOCK_MONOTONIC_COARSE
                    | CLOCK_BOOTTIME
                    | CLOCK_PROCESS_CPUTIME_ID
                    | CLOCK_THREAD_CPUTIME_ID => {
                        let secs = (uptime_ms / 1000) as i64;
                        let mut nsecs = ((uptime_ms % 1000) * 1_000_000) as i64;
                        if secs == 0 && nsecs == 0 {
                            nsecs = 1;
                        }
                        (secs, nsecs)
                    }
                    _ => return Some(Translation::Handled(-22)),
                };
                crate::syscall::user_write::<i64>(tp, secs);
                crate::syscall::user_write::<i64>(tp + 8, nsecs);
            }
            Some(Translation::Handled(0))
        }
        L_NANOSLEEP => {
            let req_ptr = args[0];
            if req_ptr == 0 {
                return Some(Translation::Handled(-14)); // EFAULT
            }

            let sec = crate::syscall::user_read::<i64>(req_ptr);
            let nsec = crate::syscall::user_read::<i64>(req_ptr + 8);
            if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
                return Some(Translation::Handled(-22)); // EINVAL
            }

            let ms = (sec as u64)
                .saturating_mul(1000)
                .saturating_add(((nsec as u64) + 999_999) / 1_000_000);
            if ms != 0 {
                let wake_tick = crate::arch::platform::uptime_ms().saturating_add(ms);
                crate::sched::sleep_until(wake_tick);
            }
            Some(Translation::Handled(0))
        }
        L_TIME => {
            let now = (super::FAKE_EPOCH + crate::arch::platform::uptime_ms() / 1000) as i64;
            if args[0] != 0 {
                crate::syscall::user_write::<i64>(args[0], now);
            }
            Some(Translation::Handled(now as isize))
        }

        // ─── System info ─────────────────────────────────────
        L_SYSINFO => {
            let result = sys_sysinfo(args[0]);
            Some(Translation::Handled(result))
        }
        L_UNAME => {
            let result = sys_uname(args[0]);
            Some(Translation::Handled(result))
        }
        L_GETRLIMIT | L_PRLIMIT64 => Some(Translation::Handled(0)), // stub
        L_GETUID | L_GETGID | L_GETEUID | L_GETEGID => Some(Translation::Handled(0)), // stub: root
        L_SET_TID_ADDR => Some(Translation::Dispatch {
            karte_nr: 115, // LINUX_SET_TID_ADDRESS
            args,
        }),
        L_SET_ROBUST_LIST | L_GET_ROBUST_LIST => Some(Translation::Handled(0)), // stub
        L_RSEQ => Some(Translation::Handled(-38)), // ENOSYS: Go gracefully degrades

        // ─── Threading ───────────────────────────────────────
        L_SET_THREAD_AREA => Some(Translation::Handled(0)), // stub
        L_ARCH_PRCTL => Some(Translation::Dispatch {
            karte_nr: super::LINUX_ARCH_PRCTL,
            args,
        }),
        L_PRCTL => Some(Translation::Handled(0)), // stub
        L_SYSLOG => Some(Translation::Dispatch {
            karte_nr: super::SYS_SYSLOG,
            args: [args[1], args[2], 0, 0, 0, 0], // skip type arg, pass buf + len
        }),

        // ─── epoll / eventfd ─────────────────────────────────
        L_EPOLL_CREATE1 => {
            // Return a fake fd
            Some(Translation::Handled(3)) // fake fd 3
        }
        L_EPOLL_CTL => Some(Translation::Handled(0)),
        L_EPOLL_WAIT | L_EPOLL_PWAIT => {
            // No events ready, return 0
            Some(Translation::Handled(0))
        }
        L_EVENTFD2 => Some(Translation::Handled(
            crate::syscall::epoll::eventfd::sys_eventfd2(args[0], args[1]),
        )),

        // ─── Misc stubs ──────────────────────────────────────
        // Note: L_POLL(7) removed — conflicts with KarteOS SYS_PIPE(7)
        L_SELECT => Some(Translation::Handled(0)),
        L_UNSHARE => Some(Translation::Handled(0)),
        L_MREMAP => Some(Translation::Handled(super::ERR_INVAL)),
        L_MINCORE => Some(Translation::Handled(0)),
        L_PTRACE => Some(Translation::Handled(super::ERR_INVAL)),
        L_TKILL | L_TGKILL => Some(Translation::Handled(0)), // stub
        L_GET_RANDOM => Some(Translation::Dispatch {
            karte_nr: 114, // LINUX_GETRANDOM
            args,
        }),
        L_GETRUSAGE => Some(Translation::Handled(0)),
        L_TIMES => Some(Translation::Handled(0)),
        L_DUP3 => Some(Translation::Dispatch {
            karte_nr: super::SYS_DUP2,
            args: [args[0], args[1], 0, 0, 0, 0],
        }),

        // ─── Network ─────────────────────────
        // Note: L_SOCKET(41), L_CONNECT(42), L_LISTEN(50) removed —
        // conflict with KarteOS SYS_MKDIR(41), SYS_UNLINK(42),
        // SYS_SETENV(50). Also L_GETSOCKNAME(51) conflicts with
        // SYS_GETENV(51), L_GETPEERNAME(52) conflicts with SYS_CHDIR(52).
        L_BIND => Some(Translation::Dispatch {
            karte_nr: super::SYS_BIND,
            args,
        }),
        L_ACCEPT => Some(Translation::Dispatch {
            karte_nr: super::SYS_ACCEPT,
            args: [args[0], 0, 0, 0, 0, 0],
        }),
        L_SENDTO => Some(Translation::Dispatch {
            karte_nr: super::SYS_SENDTO,
            args,
        }),
        L_RECVFROM => Some(Translation::Dispatch {
            karte_nr: super::SYS_RECVFROM,
            args: [args[0], args[1], args[2], 0, 0, 0],
        }),
        L_SHUTDOWN => Some(Translation::Dispatch {
            karte_nr: super::SYS_SHUTDOWN,
            args: [args[0], args[1], 0, 0, 0, 0],
        }),
        L_SETSOCKOPT | L_GETSOCKOPT | L_SENDMSG | L_RECVMSG => Some(Translation::Handled(0)),

        _ => None, // Unknown — let KarteOS dispatch handle it
    }
}

/// Count the length of a NUL-terminated string in user memory (max 256 bytes).
/// Switches to user CR3 temporarily since syscall runs under kernel CR3.
pub fn count_user_string(ptr: usize) -> usize {
    if ptr < 0x1000 {
        return 0;
    }
    let mut len = 0usize;
    while len < 256 {
        let b = super::user_read_u8(ptr + len);
        if b == 0 {
            break;
        }
        len += 1;
    }
    len
}

// ─── Linux x86_64 syscall implementations ───────────────────────

/// sys_getcwd: Get current working directory path.
///
/// Writes the CWD path (including null terminator) to the user buffer.
/// Defaults to "/" if CWD environment variable is not set.
/// Returns the number of bytes written (excluding null), or -ERR_RANGE
/// if the buffer is too small or the pointer is null.
fn sys_getcwd(buf_ptr: usize, buf_len: usize) -> isize {
    if buf_ptr == 0 || buf_len == 0 {
        return super::ERR_RANGE;
    }
    let cwd = match crate::env::get("CWD") {
        Some(s) => s,
        None => {
            // Default to "/"
            if buf_len < 2 {
                return super::ERR_RANGE;
            }
            crate::syscall::user_write::<u8>(buf_ptr, b'/');
            crate::syscall::user_write::<u8>(buf_ptr + 1, 0);
            return 1;
        }
    };
    let cwd_bytes = cwd.as_bytes();
    // Need buf_len to accommodate path + null terminator
    if cwd_bytes.len() >= buf_len {
        return super::ERR_RANGE;
    }
    crate::syscall::user_write_bytes(buf_ptr, cwd_bytes);
    // Append null terminator
    crate::syscall::user_write::<u8>(buf_ptr + cwd_bytes.len(), 0);
    cwd_bytes.len() as isize
}

/// sys_sysinfo: Fill Linux `sysinfo` struct in user memory.
///
/// The Linux sysinfo struct is 112 bytes on x86_64. We populate:
///   - totalram (u64 at offset 32): 512 MB (QEMU default)
///   - freeram (u64 at offset 40): 256 MB (half of total)
///   - mem_unit (u32 at offset 104): 1
/// All other fields remain zero.
fn sys_sysinfo(info_ptr: usize) -> isize {
    if info_ptr == 0 {
        return super::ERR_INVAL;
    }
    // Zero out the entire struct (112 bytes)
    crate::syscall::user_write_bytes(info_ptr, &[0u8; 112]);
    // totalram: 512 MB
    crate::syscall::user_write::<u64>(info_ptr + 32, 512 * 1024 * 1024);
    // freeram: 256 MB
    crate::syscall::user_write::<u64>(info_ptr + 40, 256 * 1024 * 1024);
    // mem_unit: 1
    crate::syscall::user_write::<u32>(info_ptr + 104, 1u32);
    0
}

/// sys_uname: Fill Linux utsname struct in user memory.
///
/// struct utsname has 6 fields of 65 bytes each (total 390 bytes).
/// Fields: sysname, nodename, release, version, machine, domainname.
/// Go runtime uses this to detect kernel version; without it Go panics
/// "failed to determine kernel version".
pub fn sys_uname(buf: usize) -> isize {
    if buf == 0 {
        return super::ERR_INVAL;
    }
    let fields: [&[u8]; 6] = [
        b"Linux\0",          // sysname
        b"karteos\0",        // nodename
        b"6.1.0\0",          // release (fake version to satisfy Go)
        b"#1 SMP KarteOS\0", // version
        b"x86_64\0",         // machine
        b"\0",               // domainname (empty)
    ];
    let mut offset = 0usize;
    for field in &fields {
        // Build a 65-byte padded buffer for this field
        let mut padded = [0u8; 65];
        let len = field.len().min(65);
        padded[..len].copy_from_slice(&field[..len]);
        crate::syscall::user_write_bytes(buf + offset, &padded);
        offset += 65;
    }
    0
}

/// sys_fstat: Fill Linux x86_64 struct stat (144 bytes) in user memory.
///
/// struct stat layout (x86_64):
///   offset 0:  st_dev (u64)     = 0
///   offset 8:  st_ino (u64)     = fd + 1
///   offset 16: st_nlink (u64)   = 1
///   offset 24: st_mode (u32)    = 0x81A4 (S_IFREG | 0644)
///   offset 28: st_uid (u32)     = 0
///   offset 32: st_gid (u32)     = 0
///   offset 36: __pad0 (u32)     = 0
///   offset 40: st_rdev (u64)    = 0
///   offset 48: st_size (u64)    = 0
///   offset 56: st_blksize (u64) = 4096
///   offset 64: st_blocks (u64)  = 0
///   offset 72: st_atime (i64)   = 0
///   offset 80: st_atime_nsec    = 0
///   offset 88: st_mtime (i64)   = 0
///   offset 96: st_mtime_nsec    = 0
///   offset 104: st_ctime (i64)  = 0
///   offset 112: st_ctime_nsec   = 0
///   (remaining 24 bytes reserved/zero)
fn sys_fstat(fd: i32, stat_ptr: usize) -> isize {
    if stat_ptr == 0 {
        return super::ERR_INVAL;
    }
    // Zero out the entire struct (144 bytes)
    crate::syscall::user_write_bytes(stat_ptr, &[0u8; 144]);
    // st_ino = fd + 1 (fake inode number, non-zero to be valid)
    crate::syscall::user_write::<u64>(stat_ptr + 8, (fd as u64).wrapping_add(1));
    // st_nlink = 1
    crate::syscall::user_write::<u64>(stat_ptr + 16, 1u64);
    // st_mode = S_IFREG | 0644 = 0x81A4
    crate::syscall::user_write::<u32>(stat_ptr + 24, 0x81A4u32);
    // st_blksize = 4096
    crate::syscall::user_write::<u64>(stat_ptr + 56, 4096u64);
    0
}

// ─── RISC-V Linux translation (preserved from original) ──────────

#[cfg(target_arch = "riscv64")]
fn translate_riscv(id: usize, args: [usize; 6]) -> Option<Translation> {
    use riscv_syscalls::*;

    match id {
        L_IOCTL => Some(Translation::Dispatch {
            karte_nr: super::SYS_IOCTL,
            args,
        }),
        L_OPENAT => {
            let path_ptr = args[1];
            let flags = args[2]; // Preserve flags! O_CREAT, O_RDWR, etc.
            let path_len = count_user_string(path_ptr);
            if path_len == 0 {
                return Some(Translation::Handled(super::ERR_NOENT));
            }
            #[cfg(target_arch = "x86_64")]
            {
                let _ = (path_ptr, path_len, flags); // suppress unused warnings
            }
            Some(Translation::Dispatch {
                karte_nr: super::SYS_OPEN,
                args: [path_ptr, path_len, flags, 0, 0, 0],
            })
        }
        L_CLOSE => Some(Translation::Dispatch {
            karte_nr: super::SYS_CLOSE,
            args,
        }),
        L_LSEEK => {
            let fd = args[0] as i32;
            let offset = args[1] as i64;
            let whence = args[2] as i32;
            let ext4_inode = super::get_fd_ext4_inode(fd);
            if let Some(inode) = ext4_inode {
                let file_size = crate::driver::ext4::file_size(inode).unwrap_or(0) as i64;
                let cur_pos = super::get_fd_pos(fd) as i64;
                let new_pos = match whence {
                    0 => offset,
                    1 => cur_pos + offset,
                    2 => file_size + offset,
                    _ => return Some(Translation::Handled(super::ERR_INVAL)),
                };
                if new_pos < 0 {
                    return Some(Translation::Handled(super::ERR_INVAL));
                }
                super::set_fd_pos(fd, new_pos as usize);
                Some(Translation::Handled(new_pos as isize))
            } else {
                Some(Translation::Handled(0))
            }
        }
        L_READ => Some(Translation::Dispatch {
            karte_nr: super::SYS_READ,
            args,
        }),
        L_WRITE => Some(Translation::Dispatch {
            karte_nr: super::SYS_WRITE,
            args,
        }),
        L_FSTAT => Some(Translation::Dispatch {
            karte_nr: super::LINUX_FSTAT,
            args,
        }),
        L_EXIT => Some(Translation::Dispatch {
            karte_nr: super::SYS_EXIT,
            args,
        }),
        L_EXIT_GROUP => Some(Translation::Dispatch {
            karte_nr: super::SYS_EXIT,
            args,
        }),
        L_SET_TID_ADDR => Some(Translation::Handled(args[0] as isize)),
        L_GETPID => Some(Translation::Dispatch {
            karte_nr: super::SYS_GETPID,
            args,
        }),
        L_BRK => Some(Translation::Dispatch {
            karte_nr: super::SYS_BRK,
            args,
        }),
        L_MUNMAP => Some(Translation::Dispatch {
            karte_nr: 112, // LINUX_MUNMAP
            args: [args[0], args[1], 0, 0, 0, 0],
        }),
        L_MMAP => Some(Translation::Dispatch {
            karte_nr: 110, // LINUX_MMAP
            args,
        }),
        L_MPROTECT => Some(Translation::Dispatch {
            karte_nr: 111, // LINUX_MPROTECT
            args: [args[0], args[1], args[2], 0, 0, 0],
        }),
        L_MADVISE => Some(Translation::Dispatch {
            karte_nr: 114, // LINUX_MADVISE
            args: [args[0], args[1], args[2], 0, 0, 0],
        }),
        L_SOCKET => Some(Translation::Dispatch {
            karte_nr: super::SYS_SOCKET,
            args,
        }),
        L_BIND => Some(Translation::Dispatch {
            karte_nr: super::SYS_BIND,
            args,
        }),
        L_CONNECT => Some(Translation::Dispatch {
            karte_nr: super::SYS_CONNECT,
            args,
        }),
        L_LISTEN => Some(Translation::Dispatch {
            karte_nr: super::SYS_LISTEN,
            args,
        }),
        L_ACCEPT => Some(Translation::Dispatch {
            karte_nr: super::SYS_ACCEPT,
            args,
        }),
        L_SENDTO => Some(Translation::Dispatch {
            karte_nr: super::SYS_SENDTO,
            args,
        }),
        L_RECVFROM => Some(Translation::Dispatch {
            karte_nr: super::SYS_RECVFROM,
            args,
        }),
        L_SHUTDOWN => Some(Translation::Dispatch {
            karte_nr: super::SYS_SHUTDOWN,
            args,
        }),
        L_FTRUNCATE => Some(Translation::Handled(0)),
        L_PREAD64 => {
            // pread64(fd, buf, count, offset) — read at specific offset
            let fd = args[0] as i32;
            let buf = args[1];
            let count = args[2];
            let offset = args[3];
            // Look up fd type to find Ext4File inode
            let ext4_inode = super::get_fd_ext4_inode(fd);
            if let Some(inode) = ext4_inode {
                let mut kbuf = alloc::vec![0u8; count];
                match crate::driver::ext4::read_file_at_offset(inode, offset, &mut kbuf) {
                    Ok(n) => {
                        for i in 0..n {
                            super::user_write_u8(buf + i, kbuf[i]);
                        }
                        Some(Translation::Handled(n as isize))
                    }
                    Err(_) => Some(Translation::Handled(super::ERR_IO)),
                }
            } else {
                // Fall back to regular read
                let result =
                    super::dispatch_inner(super::SYS_READ, [fd as usize, buf, count, 0, 0, 0]);
                Some(Translation::Handled(result))
            }
        }
        L_PWRITE64 => {
            let fd = args[0] as i32;
            let buf = args[1];
            let count = args[2];
            let offset = args[3];
            let ext4_inode = super::get_fd_ext4_inode(fd);
            if let Some(inode) = ext4_inode {
                let data = super::user_read_bytes(buf, count);
                match crate::driver::ext4::write_file_at_offset(inode, offset, &data) {
                    Ok(n) => Some(Translation::Handled(n as isize)),
                    Err(_) => Some(Translation::Handled(super::ERR_IO)),
                }
            } else {
                let result =
                    super::dispatch_inner(super::SYS_WRITE, [fd as usize, buf, count, 0, 0, 0]);
                Some(Translation::Handled(result))
            }
        }
        L_FSYNC => {
            let r = super::linux_fsync(args[0]);
            Some(Translation::Handled(r))
        }
        L_FALLOCATE => Some(Translation::Handled(0)),
        L_WRITEV | L_PWRITEV => {
            let fd = args[0] as i32;
            let iov = args[1];
            let iovcnt = args[2];
            let offset = if matches!(id, L_PWRITEV) {
                args[3]
            } else {
                super::get_fd_pos(fd)
            };
            let inode = match super::get_fd_ext4_inode(fd) {
                Some(i) => i,
                None => return Some(Translation::Handled(0)),
            };
            let mut pos = offset;
            let mut total = 0usize;
            for i in 0..iovcnt {
                let base = super::user_read::<usize>(iov + i * 16);
                let len = super::user_read::<usize>(iov + i * 16 + 8);
                if len == 0 {
                    continue;
                }
                let data = super::user_read_bytes(base, len);
                match crate::driver::ext4::write_file_at_offset(inode, pos, &data) {
                    Ok(n) => {
                        pos += n;
                        total += n;
                    }
                    Err(_) => break,
                }
            }
            if !matches!(id, L_PWRITEV) {
                super::set_fd_pos(fd, pos);
            }
            Some(Translation::Handled(total as isize))
        }
        L_READV | L_PREADV => {
            let fd = args[0] as i32;
            let iov = args[1];
            let iovcnt = args[2];
            let offset = if matches!(id, L_PREADV) {
                args[3]
            } else {
                super::get_fd_pos(fd)
            };
            let inode = match super::get_fd_ext4_inode(fd) {
                Some(i) => i,
                None => return Some(Translation::Handled(0)),
            };
            let mut pos = offset;
            let mut total = 0usize;
            for i in 0..iovcnt {
                let base = super::user_read::<usize>(iov + i * 16);
                let len = super::user_read::<usize>(iov + i * 16 + 8);
                if len == 0 {
                    continue;
                }
                let mut kbuf = alloc::vec![0u8; len];
                match crate::driver::ext4::read_file_at_offset(inode, pos, &mut kbuf) {
                    Ok(n) => {
                        for j in 0..n {
                            super::user_write_u8(base + j, kbuf[j]);
                        }
                        pos += n;
                        total += n;
                        if n < len {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            if !matches!(id, L_PREADV) {
                super::set_fd_pos(fd, pos);
            }
            Some(Translation::Handled(total as isize))
        }
        L_UNLINKAT => {
            let pl = count_user_string(args[1]);
            if pl > 0 {
                let name = match super::read_user_path(args[1], pl) {
                    Some(n) => n,
                    None => return Some(Translation::Handled(0)),
                };
                let name = super::resolve_path(&name);
                let _ = crate::driver::fs::delete_file(&name);
            }
            Some(Translation::Handled(0))
        }
        L_GETUID | L_GETEUID | L_GETGID | L_GETEGID => Some(Translation::Handled(0)),
        _ => None,
    }
}

// Additional RISC-V translations for Go runtime support
fn translate_riscv_go(id: usize, args: [usize; 6]) -> Option<Translation> {
    use riscv_syscalls::*;
    match id {
        L_SCHED_GETAFFINITY => {
            let size = core::cmp::min(args[1], 128);
            if args[2] != 0 && size > 0 {
                for i in 0..size {
                    super::user_write_u8(args[2] + i, 0);
                }
                super::user_write_u8(args[2], 0x01);
            }
            Some(Translation::Handled(size as isize))
        }
        L_SCHED_YIELD => Some(Translation::Dispatch {
            karte_nr: super::LINUX_SCHED_YIELD,
            args,
        }),
        L_FUTEX => Some(Translation::Dispatch {
            karte_nr: super::LINUX_FUTEX,
            args,
        }),
        L_NANOSLEEP => {
            let req = args[0];
            if req < 0x1000 {
                return Some(Translation::Handled(-14));
            }
            let sec = super::user_read::<i64>(req);
            let nsec = super::user_read::<i64>(req + 8);
            if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
                return Some(Translation::Handled(-22));
            }
            let ms = (sec as u64)
                .saturating_mul(1000)
                .saturating_add(((nsec as u64) + 999_999) / 1_000_000);
            if ms != 0 {
                crate::sched::sleep_until(crate::arch::platform::uptime_ms().saturating_add(ms));
            }
            Some(Translation::Handled(0))
        }
        L_CLOCK_GETTIME => {
            if args[1] != 0 {
                let ms = crate::arch::platform::uptime_ms();
                super::user_write::<i64>(args[1], (super::FAKE_EPOCH + ms / 1000) as i64);
                super::user_write::<i64>(args[1] + 8, ((ms % 1000) * 1_000_000) as i64);
            }
            Some(Translation::Handled(0))
        }
        L_CLOCK_GETRES => {
            if args[1] != 0 {
                super::user_write::<i64>(args[1], 0);
                super::user_write::<i64>(args[1] + 8, 1);
            }
            Some(Translation::Handled(0))
        }
        L_GETTIMEOFDAY => {
            if args[0] != 0 {
                let ms = crate::arch::platform::uptime_ms();
                super::user_write::<i64>(args[0], (super::FAKE_EPOCH + ms / 1000) as i64);
                super::user_write::<i64>(args[0] + 8, ((ms % 1000) * 1000) as i64);
            }
            Some(Translation::Handled(0))
        }
        L_UNAME => {
            let r = sys_uname(args[0]);
            Some(Translation::Handled(r))
        }
        L_SYSINFO => {
            let r = sys_sysinfo(args[0]);
            Some(Translation::Handled(r))
        }
        L_PRCTL => Some(Translation::Handled(0)),
        L_PRLIMIT64 => Some(Translation::Handled(0)),
        L_GETRANDOM => Some(Translation::Dispatch {
            karte_nr: super::LINUX_GETRANDOM,
            args,
        }),
        L_CLONE => Some(Translation::Dispatch {
            karte_nr: super::LINUX_CLONE,
            args,
        }),
        L_RT_SIGACTION => Some(Translation::Handled(0)),
        L_RT_SIGPROCMASK => Some(Translation::Handled(0)),
        L_RT_SIGRETURN => Some(Translation::Handled(0)),
        L_SIGALTSTACK => Some(Translation::Handled(0)),
        L_KILL => Some(Translation::Handled(0)),
        L_GETTID => Some(Translation::Dispatch {
            karte_nr: super::SYS_GETPID,
            args,
        }),
        L_FACCESSAT => Some(Translation::Handled(0)),
        L_FCNTL => Some(Translation::Dispatch {
            karte_nr: super::LINUX_FCNTL,
            args: [args[0], args[1], args[2], 0, 0, 0],
        }),
        L_MKDIRAT | L_MKDIRAT_OLD => {
            let pl = count_user_string(args[1]);
            if pl == 0 {
                return Some(Translation::Handled(super::ERR_NOENT));
            }
            Some(Translation::Dispatch {
                karte_nr: super::SYS_MKDIR,
                args: [args[1], pl, 0, 0, 0, 0],
            })
        }
        L_NEWFSTATAT => {
            let pl = count_user_string(args[1]);
            if pl == 0 {
                return Some(Translation::Handled(super::ERR_NOENT));
            }
            let name = match super::read_user_path(args[1], pl) {
                Some(n) => n,
                None => return Some(Translation::Handled(super::ERR_NOENT)),
            };
            let name = super::resolve_path(&name);
            if let Some(inode) = crate::driver::fs::lookup_path(&name) {
                if args[2] != 0 {
                    let is_dir = crate::driver::ext4::metadata_of(inode)
                        .map(|m| m.is_dir())
                        .unwrap_or(false);
                    let mode = if is_dir { 0x41EDu32 } else { 0x81A4u32 };
                    let _ = super::user_write::<u32>(args[2] + 16, mode);
                }
                Some(Translation::Handled(0))
            } else {
                Some(Translation::Handled(super::ERR_NOENT))
            }
        }
        L_READLINKAT => Some(Translation::Handled(super::ERR_NOENT)),
        L_GETDENTS64 => Some(Translation::Handled(0)), // stub: empty dir listing
        L_GETCWD => {
            let r = sys_getcwd(args[0], args[1]);
            Some(Translation::Handled(r))
        }
        L_CHDIR => {
            let pl = count_user_string(args[0]);
            if pl == 0 {
                return Some(Translation::Handled(super::ERR_NOENT));
            }
            Some(Translation::Dispatch {
                karte_nr: super::SYS_CHDIR,
                args: [args[0], pl, 0, 0, 0, 0],
            })
        }
        L_EPOLL_CREATE1 => Some(Translation::Dispatch {
            karte_nr: super::LINUX_EPOLL_CREATE1,
            args,
        }),
        L_EPOLL_CTL => Some(Translation::Dispatch {
            karte_nr: super::LINUX_EPOLL_CTL,
            args,
        }),
        L_EPOLL_PWAIT => Some(Translation::Dispatch {
            karte_nr: super::LINUX_EPOLL_PWAIT,
            args,
        }),
        L_EVENTFD2 => Some(Translation::Dispatch {
            karte_nr: super::LINUX_EVENTFD2,
            args,
        }),
        L_TIMERFD_CREATE => Some(Translation::Dispatch {
            karte_nr: super::LINUX_TIMERFD_CREATE,
            args,
        }),
        L_TIMERFD_SETTIME => Some(Translation::Dispatch {
            karte_nr: super::LINUX_TIMERFD_SETTIME,
            args,
        }),
        L_PIPE2 => Some(Translation::Dispatch {
            karte_nr: super::LINUX_PIPE2,
            args,
        }),
        L_DUP => Some(Translation::Dispatch {
            karte_nr: super::SYS_DUP2,
            args: [args[0], 0, 0, 0, 0, 0], // dup(fd) → dup2(fd, 0) — Go uses fcntl(F_DUPFD) anyway
        }),
        L_DUP3 => Some(Translation::Dispatch {
            karte_nr: super::LINUX_DUP3,
            args,
        }),
        _ => None,
    }
}

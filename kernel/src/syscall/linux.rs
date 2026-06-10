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
        translate_riscv(id, args)
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

        // ─── Signals ─────────────────────────────────────────
        L_RT_SIGACTION => Some(Translation::Dispatch {
            karte_nr: 102, // LINUX_RT_SIGACTION
            args,
        }),
        L_RT_SIGPROCMASK => Some(Translation::Dispatch {
            karte_nr: 103, // LINUX_RT_SIGPROCMASK
            args,
        }),
        L_RT_SIGRETURN => Some(Translation::Dispatch {
            karte_nr: 104, // LINUX_RT_SIGRETURN
            args,
        }),
        L_SIGALTSTACK => Some(Translation::Dispatch {
            karte_nr: 105, // LINUX_SIGALTSTACK
            args,
        }),

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
        L_NEWFSTATAT => Some(Translation::Handled(0)), // stub
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
        L_GETDENTS | L_GETDENTS64 => Some(Translation::Handled(0)), // stub: empty dir
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
        L_IOCTL => Some(Translation::Handled(0)),                                  // stub

        // ─── Scheduling ──────────────────────────────────────
        L_SCHED_YIELD => Some(Translation::Dispatch {
            karte_nr: 106, // LINUX_SCHED_YIELD
            args,
        }),
        L_SCHED_GETAFFINITY => {
            // sched_getaffinity(pid, size, mask) — fake: CPU 0 available
            let size = args[1]; // mask_size in bytes
            let mask_ptr = args[2] as *mut u8;
            if mask_ptr as usize != 0 && size > 0 {
                // Zero-fill the entire mask
                crate::arch::trap::with_user_cr3(|| unsafe {
                    core::ptr::write_bytes(mask_ptr, 0, size);
                    // Set CPU 0 as available (first byte = 0x01)
                    core::ptr::write_volatile(mask_ptr, 0x01u8);
                });
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
            // clock_gettime(clockid, tp) — fake
            if args[1] != 0 {
                let tp = args[1];
                crate::syscall::user_write::<u64>(tp, 0);
                crate::syscall::user_write::<u64>(tp + 8, 0);
            }
            Some(Translation::Handled(0))
        }
        L_NANOSLEEP => {
            // nanosleep(req, rem) — fake: return immediately
            Some(Translation::Handled(0))
        }
        L_TIME => {
            // time(tloc) — return 0
            if args[0] != 0 {
                crate::syscall::user_write::<u64>(args[0], 0);
            }
            Some(Translation::Handled(0))
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
    if ptr == 0 {
        return 0;
    }
    let mut len = 0usize;
    #[cfg(target_arch = "x86_64")]
    crate::arch::trap::with_user_cr3(|| {
        while len < 256 {
            let b = unsafe { core::ptr::read_volatile((ptr + len) as *const u8) };
            if b == 0 {
                break;
            }
            len += 1;
        }
    });
    #[cfg(not(target_arch = "x86_64"))]
    while len < 256 {
        let b = unsafe { core::ptr::read_volatile((ptr + len) as *const u8) };
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
        L_IOCTL => Some(Translation::Handled(0)),
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
        L_LSEEK => Some(Translation::Handled(0)),
        L_READ => Some(Translation::Dispatch {
            karte_nr: super::SYS_READ,
            args,
        }),
        L_WRITE => Some(Translation::Dispatch {
            karte_nr: super::SYS_WRITE,
            args,
        }),
        L_FSTAT => Some(Translation::Handled(0)),
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
        _ => None,
    }
}

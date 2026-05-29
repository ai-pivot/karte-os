//! Linux RISC-V syscall compatibility layer.
//!
//! Provides transparent translation from Linux syscall numbers to KarteOS
//! equivalents. Enabled at runtime via a global switch.
//!
//! ## Design
//!
//! - **Runtime opt-in**: controlled by `ENABLED` atomic bool
//! - **Zero intrusion**: existing KarteOS syscall handlers are completely unchanged;
//!   the translation layer sits *above* `dispatch()` as a transparent filter
//! - **Translation table**: const sorted array, binary-searched at runtime
//! - **Argument adaptation**: some Linux syscalls have different argument layouts;
//!   the `translate()` function adjusts args in-place where needed
//!
//! ## Supported Linux syscalls
//!
//! | Linux nr | Name        | KarteOS nr | Notes                           |
//! |----------|-------------|------------|---------------------------------|
//! | 63       | read        | 3          | 1:1 mapping                     |
//! | 64       | write       | 2          | 1:1 mapping                     |
//! | 93       | exit        | 1          | 1:1 mapping                     |
//! | 94       | exit_group  | 1          | maps to exit                    |
//! | 172      | getpid      | 5          | 1:1 mapping                     |
//! | 214      | brk         | 4          | 1:1 mapping                     |
//! | 222      | mmap        | 6          | args adapted                    |

use core::sync::atomic::{AtomicBool, Ordering};

/// Global runtime switch for the Linux compatibility layer.
/// When `false` (default), translation is a no-op and incurs zero overhead
/// (the dispatch match runs directly on the original syscall number).
/// Set to `true` to enable translation for Linux-compiled ELFs.
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

// ─── Syscall number constants (Linux RISC-V) ────────────────────────

const L_CLOSE: usize = 57;
const L_LSEEK: usize = 62;
const L_READ: usize = 63;
const L_WRITE: usize = 64;
const L_FSTAT: usize = 80;
const L_EXIT: usize = 93;
const L_EXIT_GROUP: usize = 94;
const L_OPENAT: usize = 56;
const L_GETPID: usize = 172;
const L_BRK: usize = 214;
const L_MUNMAP: usize = 215;
const L_MMAP: usize = 222;
const L_IOCTL: usize = 29;
const L_SET_TID_ADDR: usize = 96;

// ─── Translation table ──────────────────────────────────────────────

/// A single entry in the Linux→KarteOS translation table.
struct Entry {
    linux_nr: usize,
    karte_nr: usize,
}

/// Sorted by `linux_nr` for binary search.
/// DO NOT reorder — binary search requires sorted order.
#[rustfmt::skip]
const TABLE: &[Entry] = &[
    Entry { linux_nr: L_IOCTL,        karte_nr: 0 },  // 29 → stub
    Entry { linux_nr: L_OPENAT,       karte_nr: 10 }, // 56 → SYS_OPEN
    Entry { linux_nr: L_CLOSE,        karte_nr: 11 }, // 57 → SYS_CLOSE
    Entry { linux_nr: L_LSEEK,        karte_nr: 3 },  // 62 → stub
    Entry { linux_nr: L_READ,         karte_nr: 3 },  // 63 → SYS_READ
    Entry { linux_nr: L_WRITE,        karte_nr: 2 },  // 64 → SYS_WRITE
    Entry { linux_nr: L_FSTAT,        karte_nr: 0 },  // 80 → stub
    Entry { linux_nr: L_EXIT,         karte_nr: 1 },  // 93 → SYS_EXIT
    Entry { linux_nr: L_EXIT_GROUP,   karte_nr: 1 },  // 94 → SYS_EXIT
    Entry { linux_nr: L_SET_TID_ADDR, karte_nr: 5 },  // 96 → SYS_GETPID
    Entry { linux_nr: L_GETPID,       karte_nr: 5 },  // 172 → SYS_GETPID
    Entry { linux_nr: L_BRK,          karte_nr: 4 },  // 214 → SYS_BRK
    Entry { linux_nr: L_MUNMAP,       karte_nr: 0 },  // 215 → stub
    Entry { linux_nr: L_MMAP,         karte_nr: 6 },  // 222 → SYS_MMAP
];

// ─── Argument adaptation ─────────────────────────────────────────────

/// Linux syscalls whose argument layout differs from KarteOS.
/// Returns `Some(remapped_args)` if adaptation is needed, `None` for 1:1 passthrough.
fn adapt_args(linux_nr: usize, args: [usize; 6]) -> Option<[usize; 6]> {
    match linux_nr {
        L_MMAP => {
            // Linux mmap(addr, len, prot, flags, fd, offset)
            // KarteOS mmap(addr, len, flags)
            // We pass prot as flags — sufficient for anonymous mappings.
            Some([args[0], args[1], args[2], 0, 0, 0])
        }
        L_OPENAT => {
            // Linux openat(dirfd, pathname, flags, mode)
            // KarteOS open(path, path_len, flags)
            // dirfd is ignored (AT_FDCWD = -100), pass pathname and flags through
            // We need the path length — but we don't know it here.
            // Instead, we route to a dedicated linux_openat handler.
            None // handled by linux_dispatch fallback
        }
        _ => None, // 1:1 passthrough
    }
}

/// Linux syscalls that have no KarteOS equivalent but need a safe stub response.
/// Returns `Some(retval)` for known stubs, `None` otherwise.
pub fn stub_dispatch(linux_nr: usize, args: [usize; 6]) -> Option<isize> {
    match linux_nr {
        L_FSTAT => Some(0),                       // fake success
        L_IOCTL => Some(0),                       // fake success
        L_MUNMAP => Some(0),                      // fake success
        L_SET_TID_ADDR => Some(args[0] as isize), // return tid
        L_LSEEK => Some(0),                       // fake success (offset = 0)
        _ => None,
    }
}

/// Handle openat(dirfd, pathname, flags, mode) → KartenOS open(path, path_len, flags)
fn linux_openat(dirfd: i32, path_ptr: usize, _flags: usize, _mode: usize) -> isize {
    let _ = dirfd; // ignore AT_FDCWD
    // Find path length (scan for NUL)
    let mut path_len = 0usize;
    while path_len < 256 {
        let b = unsafe { core::ptr::read_volatile((path_ptr + path_len) as *const u8) };
        if b == 0 {
            break;
        }
        path_len += 1;
    }
    if path_len == 0 {
        return -1;
    }
    // Route to KarteOS sys_open (syscall 10)
    super::sys_open(path_ptr, path_len, 0)
}

// ─── Public API ──────────────────────────────────────────────────────

/// Result of a successful syscall translation.
pub enum Translation {
    /// Translate to a KarteOS syscall number and (possibly adapted) args.
    Dispatch { karte_nr: usize, args: [usize; 6] },
    /// Handle entirely within the compat layer (no KarteOS dispatch needed).
    Handled(isize),
}

/// Try to translate a syscall number + arguments from Linux to KarteOS.
///
/// Returns `None` when:
/// - The compat layer is disabled (`ENABLED == false`)
/// - The syscall number is not in the translation table
///
/// The caller should pass the original `(id, args)` through to the
/// normal KarteOS dispatch when this returns `None`.
pub fn translate(id: usize, args: [usize; 6]) -> Option<Translation> {
    if !is_enabled() {
        return None;
    }

    // Check stub dispatch first (syscalls with no KarteOS equivalent)
    if let Some(retval) = stub_dispatch(id, args) {
        return Some(Translation::Handled(retval));
    }

    // Handle openat specially (different argument convention)
    if id == L_OPENAT {
        let result = linux_openat(args[0] as i32, args[1], args[2], args[3]);
        return Some(Translation::Handled(result));
    }

    // Binary search over the sorted table
    let idx = TABLE
        .binary_search_by(|entry| entry.linux_nr.cmp(&id))
        .ok()?;

    let entry = &TABLE[idx];
    let translated_args = adapt_args(id, args).unwrap_or(args);

    Some(Translation::Dispatch {
        karte_nr: entry.karte_nr,
        args: translated_args,
    })
}

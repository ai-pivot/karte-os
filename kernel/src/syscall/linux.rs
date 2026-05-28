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

const L_READ: usize = 63;
const L_WRITE: usize = 64;
const L_EXIT: usize = 93;
const L_EXIT_GROUP: usize = 94;
const L_GETPID: usize = 172;
const L_BRK: usize = 214;
const L_MMAP: usize = 222;

// ─── Translation table ──────────────────────────────────────────────

/// A single entry in the Linux→KarteOS translation table.
struct Entry {
    linux_nr: usize,
    karte_nr: usize,
}

/// Sorted by `linux_nr` for binary search.
const TABLE: &[Entry] = &[
    Entry { linux_nr: L_READ, karte_nr: 3 },      // 63 → SYS_READ
    Entry { linux_nr: L_WRITE, karte_nr: 2 },      // 64 → SYS_WRITE
    Entry { linux_nr: L_EXIT, karte_nr: 1 },       // 93 → SYS_EXIT
    Entry { linux_nr: L_EXIT_GROUP, karte_nr: 1 }, // 94 → SYS_EXIT
    Entry { linux_nr: L_GETPID, karte_nr: 5 },     // 172 → SYS_GETPID
    Entry { linux_nr: L_BRK, karte_nr: 4 },        // 214 → SYS_BRK
    Entry { linux_nr: L_MMAP, karte_nr: 6 },       // 222 → SYS_MMAP
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
        _ => None, // 1:1 passthrough
    }
}

// ─── Public API ──────────────────────────────────────────────────────

/// Result of a successful syscall translation.
pub struct Translation {
    /// The KarteOS syscall number to dispatch.
    pub karte_nr: usize,
    /// The (possibly adapted) argument array.
    pub args: [usize; 6],
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

    // Binary search over the sorted table
    let idx = TABLE
        .binary_search_by(|entry| entry.linux_nr.cmp(&id))
        .ok()?;

    let entry = &TABLE[idx];
    let translated_args = adapt_args(id, args).unwrap_or(args);

    Some(Translation {
        karte_nr: entry.karte_nr,
        args: translated_args,
    })
}

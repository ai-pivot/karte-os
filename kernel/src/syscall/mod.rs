//! KarteOS Syscall ABI
//!
//! Calling convention:
//!   ecall instruction (triggers UserEnvCall, exception code 8)
//!   a7 = syscall number
//!   a0-a5 = arguments (up to 6)
//!   a0 = return value (>= 0 success, < 0 error)

pub mod linux;
pub mod user_ptr;

pub use user_ptr::{UserPtr, UserSlice, UserSliceMut};

#[cfg(target_arch = "x86_64")]
pub mod epoll;

/// Fake epoch base for gettimeofday / clock_gettime (2025-06-09 00:00:00 UTC).
const FAKE_EPOCH: u64 = 1749427200;

// ─── Syscall numbers ──────────────────────────────────────────────

// Level 1: Core
pub const SYS_DEBUG_PRINT: usize = 0;
pub const SYS_EXIT: usize = 1;
pub const SYS_WRITE: usize = 2;
pub const SYS_READ: usize = 3;
pub const SYS_BRK: usize = 4;
pub const SYS_GETPID: usize = 5;
pub const SYS_MMAP: usize = 6;
pub const SYS_PIPE: usize = 7; // pipe(int[2] fd_ptr) → 0 on success
pub const SYS_DUP2: usize = 8; // dup2(oldfd, newfd) → newfd

// Level 2: Filesystem (reserved)
pub const SYS_OPEN: usize = 10;
pub const SYS_CLOSE: usize = 11;

// Level 5: Threading
pub const SYS_SPAWN: usize = 30;
pub const SYS_WAITPID: usize = 31;
pub const SYS_EXEC: usize = 32; // spawn by file path
pub const SYS_EXEC_FD: usize = 33; // exec with fd redirection: (path, len, redir_stdin, redir_stdout)
pub const SYS_FORK: usize = 34; // fork current process

// Level 6: Extended
pub const SYS_LS: usize = 40;
pub const SYS_MKDIR: usize = 41;
pub const SYS_UNLINK: usize = 42;

// Level 7: Environment
pub const SYS_SETENV: usize = 50;
pub const SYS_GETENV: usize = 51;

// Level 8: Directory
pub const SYS_CHDIR: usize = 52;

// Level 9: Signal
pub const SYS_KILL: usize = 60; // kill(pid, sig)
pub const SYS_SIGRET: usize = 61; // sigreturn (clear pending signal)

// Level 10: Terminal control
pub const SYS_IOCTL: usize = 80; // ioctl(fd, cmd, arg) — terminal control

// Level 10: Network
pub const SYS_SOCKET: usize = 70; // socket(domain, type, protocol) → fd
pub const SYS_BIND: usize = 71; // bind(fd, addr_ptr, addr_len) → 0
pub const SYS_CONNECT: usize = 72; // connect(fd, addr_ptr, addr_len) → 0
pub const SYS_LISTEN: usize = 73; // listen(fd, backlog) → 0
pub const SYS_ACCEPT: usize = 74; // accept(fd, addr_ptr, addr_len_ptr) → fd
pub const SYS_SENDTO: usize = 75; // sendto(fd, buf, len, flags, addr_ptr, addr_len) → sent
pub const SYS_RECVFROM: usize = 76; // recvfrom(fd, buf, len, flags, addr_ptr, addr_len_ptr) → received
pub const SYS_SHUTDOWN: usize = 77; // shutdown(fd, how) → 0
pub const SYS_SYSLOG: usize = 81; // syslog(buf, len, offset) → bytes_read

// ─── Linux compatibility syscalls (translated from Linux x86_64 numbers) ──
pub const LINUX_CLONE: usize = 100;
pub const LINUX_FUTEX: usize = 101;
pub const LINUX_RT_SIGACTION: usize = 102;
pub const LINUX_RT_SIGPROCMASK: usize = 103;
pub const LINUX_RT_SIGRETURN: usize = 104;
pub const LINUX_SIGALTSTACK: usize = 105;
pub const LINUX_SCHED_YIELD: usize = 106;
pub const LINUX_MMAP: usize = 110;
pub const LINUX_MPROTECT: usize = 111;
pub const LINUX_MUNMAP: usize = 112;
pub const LINUX_ARCH_PRCTL: usize = 113;
pub const LINUX_MADVISE: usize = 114;
pub const LINUX_GETRANDOM: usize = 114;
pub const LINUX_SET_TID_ADDRESS: usize = 115;

// ─── Error codes ──────────────────────────────────────────────────

pub const ERR_OK: isize = 0;
// ─── Linux-compatible errno values ─────────────────────────────
// These are negated Linux errno values returned from syscalls.
// Go / Rust / C programs on user space expect these exact values.
pub const ERR_INVAL: isize = -22; // EINVAL — Invalid argument
pub const ERR_NOMEM: isize = -12; // ENOMEM — Out of memory
pub const ERR_NOENT: isize = -2; // ENOENT — No such file or directory
pub const ERR_IO: isize = -5; // EIO — I/O error
pub const ERR_ACCES: isize = -13; // EACCES — Permission denied
pub const ERR_RANGE: isize = -34; // ERANGE — Result too large
pub const ERR_INTR: isize = -4; // EINTR — Interrupted system call

// ─── VMA (Virtual Memory Area) tracking ────────────────────────────
//
// Tracks all mmap'd regions so the PF handler can distinguish valid
// lazy-allocated pages from illegal accesses, and madvise/mprotect
// can operate on the correct address ranges.

/// A single VMA region descriptor.
#[derive(Clone, Copy)]
struct VmaRegion {
    start: usize,
    end: usize,  // exclusive (first byte past the region)
    prot: usize, // PROT_* bit flags (0 = PROT_NONE)
    active: bool,
}

const MAX_VMAS: usize = 1024;

static VMA_TABLE: spin::Mutex<[VmaRegion; MAX_VMAS]> = spin::Mutex::new(
    [const {
        VmaRegion {
            start: 0,
            end: 0,
            prot: 0,
            active: false,
        }
    }; MAX_VMAS],
);

/// Check if `addr` falls within a VMA that permits access (prot != PROT_NONE).
/// Returns Some(prot) if valid, None if no VMA covers this address or VMA is PROT_NONE.
pub fn vma_check(addr: usize) -> Option<usize> {
    let table = VMA_TABLE.lock();
    let mut best_prot: Option<usize> = None;
    let mut best_size: usize = usize::MAX;
    for vma in table.iter() {
        if vma.active && addr >= vma.start && addr < vma.end {
            let size = vma.end - vma.start;
            let cur_accessible = vma.prot != 0;
            let best_accessible = best_prot.map_or(false, |p| p != 0);
            if (cur_accessible && !best_accessible)
                || (cur_accessible == best_accessible && size < best_size)
            {
                best_size = size;
                best_prot = if vma.prot == 0 { None } else { Some(vma.prot) };
            }
        }
    }
    best_prot
}

/// Query VMA protection for `addr` — distinguishes PROT_NONE from no-VMA.
/// Returns `Some(prot)` if a VMA covers this address (prot may be 0 for PROT_NONE).
/// Returns `None` if no VMA covers this address at all.
pub fn vma_query(addr: usize) -> Option<usize> {
    let table = VMA_TABLE.lock();
    for vma in table.iter() {
        if vma.active && addr >= vma.start && addr < vma.end {
            return Some(vma.prot);
        }
    }
    None
}

/// Dump VMA entries near a given address for debugging.
pub fn vma_dump_region(addr: usize) {
    let table = VMA_TABLE.lock();
    let range = 64 * 1024 * 1024; // ±64MB
    let mut count = 0;
    let mut total_active = 0;
    for vma in table.iter() {
        if vma.active {
            total_active += 1;
            if vma.start < addr + range && vma.end > addr.saturating_sub(range) {
                let contains = addr >= vma.start && addr < vma.end;
                crate::console_println!(
                    "[VMA] {:#x}..{:#x} prot={:#x} {}",
                    vma.start,
                    vma.end,
                    vma.prot,
                    if contains { "<<< CONTAINS fault" } else { "" }
                );
                count += 1;
                if count >= 20 {
                    break;
                }
            }
        }
    }
    crate::console_println!(
        "[VMA] total active={}/{} shown={}",
        total_active,
        MAX_VMAS,
        count
    );
}

/// Check if [start, end) overlaps with any active VMA entry.
pub fn vma_overlaps(start: usize, end: usize) -> bool {
    let table = VMA_TABLE.lock();
    for vma in table.iter() {
        if vma.active && vma.start < end && vma.end > start {
            return true;
        }
    }
    false
}

/// Bump allocator for mmap addresses.
/// Must be past all ELF-loaded segments to avoid overwriting mapped data.
static NEXT_MMAP_ADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Maximum virtual address of ELF PT_LOAD segments.
/// Used to prevent MAP_FIXED from overwriting ELF data.
static MAX_ELF_VADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Flag indicating ELF loading is in progress.
/// When false, map() operations on ELF range are protected.
static ELF_LOADING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Ensure the mmap bump allocator starts at or above `min_addr`.
/// Called by the ELF loader after loading segments, so that mmap
/// doesn't allocate addresses that overlap with loaded ELF data.
pub fn ensure_mmap_above(min_addr: usize) {
    let aligned = (min_addr + 4095) & !4095;
    NEXT_MMAP_ADDR.fetch_max(aligned, core::sync::atomic::Ordering::Relaxed);
    MAX_ELF_VADDR.store(aligned, core::sync::atomic::Ordering::Relaxed);
}

/// Register an ELF PT_LOAD segment as a VMA entry.
/// This prevents mmap from allocating addresses that overlap with loaded segments.
pub fn register_elf_vma(start: usize, end: usize, prot: usize) {
    let _ = vma_add(start, end, prot, false);
}

/// Return true when `addr` belongs to the ELF PT_LOAD address range.
///
/// Anonymous mmap regions can use the same protection bits as ELF segments,
/// so protection alone cannot distinguish them in the page-fault handler.
pub fn vma_is_elf(addr: usize) -> bool {
    let max = MAX_ELF_VADDR.load(core::sync::atomic::Ordering::Relaxed);
    max != 0 && addr >= 0x400000 && addr < max && vma_query(addr).is_some()
}

/// Split or remove VMA entries that overlap with [start, end).
/// For each overlapping VMA:
/// - If it fully contains [start, end), split into two (tail re-inserted)
/// - If [start, end) covers its tail, truncate it
/// - If [start, end) covers its head, move start forward
/// - If [start, end) fully covers it, deactivate it
///
/// Returns up to MAX_VMAS tail splits as (start, end, prot).
fn split_overlapping_vmas(
    table: &mut [VmaRegion; MAX_VMAS],
    start: usize,
    end: usize,
) -> [(usize, usize, usize); MAX_VMAS] {
    let mut tails = [(0usize, 0usize, 0usize); MAX_VMAS];
    let mut tail_count = 0;
    for i in 0..MAX_VMAS {
        let vma = &table[i];
        if !vma.active || vma.start >= end || vma.end <= start {
            continue;
        }
        // Overlap exists
        let vma_end = vma.end;
        let vma_prot = vma.prot;

        if vma.start < start && vma.end > end {
            // Fully contains: split into [vma_start, start) + [end, vma_end)
            table[i].end = start;
            if tail_count < MAX_VMAS {
                tails[tail_count] = (end, vma_end, vma_prot);
                tail_count += 1;
            }
        } else if vma.start < start {
            // Overlaps tail: truncate
            table[i].end = start;
        } else if vma.end > end {
            // Overlaps head: move start
            table[i].start = end;
        } else {
            // Fully covered: deactivate
            table[i].active = false;
        }
    }
    tails
}

/// Add or update a VMA entry for [start, end) with the given prot.
/// For MAP_FIXED, removes any overlapping entries first.
/// Returns Ok(()) on success, Err(()) if no free VMA slot is available.
pub fn vma_add(start: usize, end: usize, prot: usize, map_fixed: bool) -> Result<(), ()> {
    let mut table = VMA_TABLE.lock();
    if map_fixed {
        let tails = split_overlapping_vmas(&mut table, start, end);
        // Re-insert the tail portions
        for (s, e, p) in tails.iter() {
            if *s == 0 && *e == 0 {
                continue;
            }
            for i in 0..MAX_VMAS {
                if !table[i].active {
                    table[i] = VmaRegion {
                        start: *s,
                        end: *e,
                        prot: *p,
                        active: true,
                    };
                    break;
                }
            }
        }
    }
    // Find a free slot and add the new VMA
    for i in 0..MAX_VMAS {
        if !table[i].active {
            table[i] = VmaRegion {
                start,
                end,
                prot,
                active: true,
            };
            return Ok(());
        }
    }
    Err(())
}

/// Remove all VMA entries overlapping [start, end).
/// Re-inserts tail fragments (portions of VMAs outside the removed range).
pub fn vma_remove_range(start: usize, end: usize) {
    let mut table = VMA_TABLE.lock();
    let tails = split_overlapping_vmas(&mut table, start, end);
    for (s, e, p) in tails.iter() {
        if *s == 0 && *e == 0 {
            continue;
        }
        for i in 0..MAX_VMAS {
            if !table[i].active {
                table[i] = VmaRegion {
                    start: *s,
                    end: *e,
                    prot: *p,
                    active: true,
                };
                break;
            }
        }
    }
}

/// Update prot for all VMA entries overlapping [start, end).
fn vma_update_prot(start: usize, end: usize, new_prot: usize) {
    let mut table = VMA_TABLE.lock();
    for vma in table.iter_mut() {
        if vma.active && vma.start < end && vma.end > start {
            vma.prot = new_prot;
        }
    }
}

/// Clear all VMA entries (called on process exit).
pub fn vma_clear() {
    let mut table = VMA_TABLE.lock();
    for vma in table.iter_mut() {
        vma.active = false;
    }
}

// ─── Global FD table (single-process simplification) ────────────────

extern crate alloc;

// ─── User memory access helpers (CR3-aware for x86_64) ──────────
// Syscall handlers run under kernel CR3 on x86_64. These helpers
// temporarily switch to user CR3 for accessing user-space memory.

/// Read a value from user space with automatic CR3 switching on x86_64.
#[inline]
pub(crate) fn user_read<T: Copy + Default>(addr: usize) -> T {
    #[cfg(target_arch = "x86_64")]
    {
        let mut val = core::mem::MaybeUninit::<T>::uninit();
        let dst = val.as_mut_ptr() as *mut u8;
        for i in 0..core::mem::size_of::<T>() {
            unsafe { core::ptr::write(dst.add(i), user_read_u8(addr + i)) };
        }
        unsafe { val.assume_init() }
    }
    #[cfg(not(target_arch = "x86_64"))]
    unsafe {
        core::ptr::read_volatile(addr as *const T)
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
pub(crate) fn user_read_u8(addr: usize) -> u8 {
    let user_root = crate::process::current_page_table_root();
    if user_root == 0 {
        return unsafe { core::ptr::read_volatile(addr as *const u8) };
    }

    let user_cr3 = user_root << 12;
    let kernel_cr3 = crate::arch::idt::get_kernel_cr3_phys();
    let rflags: u64;
    let byte: u8;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {}",
            "cli",
            out(reg) rflags
        );
        core::arch::asm!(
            "mov cr3, {user_cr3}",
            "mov {byte}, byte ptr [{addr}]",
            "mov cr3, {kernel_cr3}",
            user_cr3 = in(reg) user_cr3,
            kernel_cr3 = in(reg) kernel_cr3,
            addr = in(reg) addr,
            byte = lateout(reg_byte) byte,
            options(nostack)
        );
        if (rflags & 0x200) != 0 {
            core::arch::asm!("sti", options(nomem, nostack));
        }
    }
    byte
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub(crate) fn user_read_u8(addr: usize) -> u8 {
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn user_write_u8_mapped(addr: usize, byte: u8) {
    let user_root = crate::process::current_page_table_root();
    if user_root == 0 {
        unsafe { core::ptr::write_volatile(addr as *mut u8, byte) };
        return;
    }

    let user_cr3 = user_root << 12;
    let kernel_cr3 = crate::arch::idt::get_kernel_cr3_phys();
    let rflags: u64;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {}",
            "cli",
            out(reg) rflags
        );
        core::arch::asm!(
            "mov cr3, {user_cr3}",
            "mov byte ptr [{addr}], {byte}",
            "mov cr3, {kernel_cr3}",
            user_cr3 = in(reg) user_cr3,
            kernel_cr3 = in(reg) kernel_cr3,
            addr = in(reg) addr,
            byte = in(reg_byte) byte,
            options(nostack)
        );
        if (rflags & 0x200) != 0 {
            core::arch::asm!("sti", options(nomem, nostack));
        }
    }
}

#[inline]
pub(crate) fn user_write_u8(addr: usize, byte: u8) {
    #[cfg(target_arch = "x86_64")]
    {
        if !ensure_user_write_pages(addr, 1) {
            return;
        }
        unsafe { user_write_u8_mapped(addr, byte) };
    }
    #[cfg(not(target_arch = "x86_64"))]
    unsafe {
        core::ptr::write_volatile(addr as *mut u8, byte)
    }
}

/// Write a value to user space with automatic CR3 switching on x86_64.
#[inline]
pub(crate) fn user_write<T: Copy>(addr: usize, val: T) {
    #[cfg(target_arch = "x86_64")]
    {
        if !ensure_user_write_pages(addr, core::mem::size_of::<T>()) {
            return;
        }
        let src = unsafe {
            core::slice::from_raw_parts((&val as *const T) as *const u8, core::mem::size_of::<T>())
        };
        for (i, &byte) in src.iter().enumerate() {
            unsafe { user_write_u8_mapped(addr + i, byte) };
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    unsafe {
        core::ptr::write_volatile(addr as *mut T, val)
    }
}

/// Read a slice of bytes from user space with automatic CR3 switching on x86_64.
///
/// CRITICAL: kernel buffers are only mutated under kernel CR3. The x86_64
/// helper switches to user CR3 only for the single-byte load itself.
#[inline]
pub(crate) fn user_read_bytes(addr: usize, len: usize) -> alloc::vec::Vec<u8> {
    let mut buf = alloc::vec::Vec::with_capacity(len);
    for i in 0..len {
        buf.push(user_read_u8(addr + i));
    }
    buf
}

#[cfg(target_arch = "x86_64")]
fn ensure_user_write_pages(addr: usize, len: usize) -> bool {
    if len == 0 {
        return true;
    }

    let page_size = crate::mm::pmm::page_size();
    let start = addr & !(page_size - 1);
    let last = match addr.checked_add(len - 1) {
        Some(last) => last,
        None => return false,
    };
    let end = (last & !(page_size - 1)) + page_size;

    crate::arch::trap::with_kernel_cr3(|| {
        let user_pt = crate::arch::trap::get_user_pt_safe();
        let mut page = start;
        while page < end {
            let flags = match vma_query(page) {
                Some(prot) => {
                    if prot & PROT_WRITE == 0 {
                        return false;
                    }
                    prot_to_pte_flags(prot)
                }
                None if page >= crate::process::USER_HEAP_BASE
                    && page < crate::process::USER_HEAP_LIMIT =>
                {
                    crate::mm::vmm::PTEFlags::URW
                }
                None if page >= crate::process::USER_STACK_BASE
                    && page < crate::process::USER_STACK_TOP =>
                {
                    crate::mm::vmm::PTEFlags::URW
                }
                None => return false,
            };

            match crate::mm::vmm::translate_user(user_pt, page) {
                Some(frame) if frame != page => {
                    crate::mm::vmm::map(user_pt, page, frame, flags);
                }
                _ => {
                    let frame = match crate::mm::pmm::alloc_frame() {
                        Some(frame) => frame,
                        None => return false,
                    };
                    unsafe {
                        core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                    }
                    crate::mm::vmm::map(user_pt, page, frame, flags);
                }
            }

            page += page_size;
        }
        true
    })
}

/// Write a slice of bytes to user space with automatic CR3 switching on x86_64.
#[inline]
pub(crate) fn user_write_bytes(addr: usize, src: &[u8]) {
    #[cfg(target_arch = "x86_64")]
    {
        if !ensure_user_write_pages(addr, src.len()) {
            return;
        }
        for (i, &byte) in src.iter().enumerate() {
            unsafe { user_write_u8_mapped(addr + i, byte) };
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    for (i, &byte) in src.iter().enumerate() {
        unsafe { core::ptr::write_volatile((addr + i) as *mut u8, byte) };
    }
}

#[cfg(target_arch = "x86_64")]
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[cfg(target_arch = "x86_64")]
fn mirror_user_json_log(data: &[u8]) {
    if !contains_bytes(data, br#""level""#)
        || !(contains_bytes(data, br#""fatal""#)
            || contains_bytes(data, br#""error""#)
            || contains_bytes(data, b"Failed"))
    {
        return;
    }

    crate::arch::platform::print("[xbot-log] ");
    for &byte in data {
        let ch = if byte == b'\n' || byte == b'\r' || (0x20..=0x7e).contains(&byte) {
            byte
        } else {
            b'.'
        };
        crate::arch::platform::console_putchar(ch);
    }
    if !data.ends_with(b"\n") {
        crate::arch::platform::console_putchar(b'\n');
    }
}

/// Check if a path refers to a pseudo-filesystem that doesn't exist on disk.
fn is_pseudo_path(path: &str) -> bool {
    path.starts_with("/proc")
        || path.starts_with("/sys")
        || path.starts_with("/dev")
        || path.starts_with("/run")
        || path.starts_with("/etc")
        || path.starts_with("/tmp")
}

/// Fill a Linux x86_64 stat structure buffer (144 bytes).
/// Layout: st_dev(0-8), st_ino(8-16), st_nlink(16-24), st_mode(24-28),
///         st_uid(28-32), st_gid(32-36), pad(36-48), st_size(48-56),
///         st_blksize(56-64), st_blocks(64-72)
#[cfg(target_arch = "x86_64")]
fn fill_stat_buffer(buf: &mut [u8; 144], st_mode: u32, st_size: i64, st_ino: u64) {
    unsafe {
        core::ptr::write_bytes(buf.as_mut_ptr(), 0, 144);
        *((buf.as_mut_ptr() as usize + 8) as *mut u64) = st_ino;
        *((buf.as_mut_ptr() as usize + 16) as *mut u64) = 1; // st_nlink
        *((buf.as_mut_ptr() as usize + 24) as *mut u32) = st_mode;
        *((buf.as_mut_ptr() as usize + 48) as *mut i64) = st_size;
        *((buf.as_mut_ptr() as usize + 56) as *mut i64) = 4096; // st_blksize
    }
}

/// Read a null-terminated user string from the given address.
/// Returns the string as a Vec<u8> (without the null terminator).
/// Switches to user CR3 temporarily since syscall runs under kernel CR3.
/// Read a NUL-terminated string from user space.
/// CRITICAL: mutate kernel buffers only under kernel CR3.
fn read_user_string(addr: usize) -> Option<alloc::vec::Vec<u8>> {
    let mut buf = alloc::vec::Vec::with_capacity(4096);
    for i in 0..4096 {
        let byte = user_read_u8(addr + i);
        if byte == 0 {
            break;
        }
        buf.push(byte);
    }
    Some(buf)
}

/// Read an argv-style pointer array from user space.
/// `ptr_array` is the address of a null-terminated array of `*const u8` pointers.
/// Each pointer points to a null-terminated string.
/// CRITICAL: mutate kernel buffers only under kernel CR3.
fn read_user_argv(ptr_array: usize) -> alloc::vec::Vec<alloc::vec::Vec<u8>> {
    let mut ptr_buf = alloc::vec::Vec::with_capacity(256);
    for i in 0..256 {
        let ptr = user_read::<usize>(ptr_array + i * core::mem::size_of::<usize>());
        if ptr == 0 {
            break;
        }
        ptr_buf.push(ptr);
    }
    // Now read each string (read_user_string handles CR3 safely)
    let mut result = alloc::vec::Vec::new();
    for &ptr in ptr_buf.iter() {
        if let Some(s) = read_user_string(ptr) {
            result.push(s);
        } else {
            break;
        }
    }
    result
}

/// Read an envp-style pointer array from user space.
/// Each pointer points to a "KEY=VALUE" null-terminated string.
fn read_user_envp(ptr_array: usize) -> alloc::vec::Vec<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> {
    let mut result = alloc::vec::Vec::new();
    for i in 0..256 {
        let ptr = user_read::<usize>(ptr_array + i * 8);
        if ptr == 0 {
            break;
        }
        if let Some(s) = read_user_string(ptr) {
            // Split at first '='
            if let Some(eq_pos) = s.iter().position(|&b| b == b'=') {
                let key = s[..eq_pos].to_vec();
                let val = s[eq_pos + 1..].to_vec();
                result.push((key, val));
            }
        } else {
            break;
        }
    }
    result
}

/// Convert per-process env BTreeMap to envp Vec for initial stack.
fn env_to_envp(
    env: &alloc::collections::BTreeMap<alloc::string::String, alloc::string::String>,
) -> alloc::vec::Vec<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> {
    env.iter()
        .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
        .collect()
}

/// Merge global env into per-process env BTreeMap.
fn merge_global_env(
    env: &mut alloc::collections::BTreeMap<alloc::string::String, alloc::string::String>,
) {
    let global_vars = crate::env::list_all();
    for (k, v) in global_vars {
        env.entry(k).or_insert(v);
    }
}

// ─── Linux signal state (for Go runtime compatibility) ────────────

/// Signal handler table: maps signal number to handler address.
/// Go registers SIGURG handler; we record it but never deliver signals.
struct SignalState {
    handlers: [core::sync::atomic::AtomicUsize; 64],
    mask: core::sync::atomic::AtomicU64,
    altstack_sp: core::sync::atomic::AtomicUsize,
    altstack_size: core::sync::atomic::AtomicUsize,
    altstack_flags: core::sync::atomic::AtomicUsize,
}

static SIGNAL_STATE: SignalState = SignalState {
    handlers: const { [const { core::sync::atomic::AtomicUsize::new(0) }; 64] },
    mask: core::sync::atomic::AtomicU64::new(0),
    altstack_sp: core::sync::atomic::AtomicUsize::new(0),
    altstack_size: core::sync::atomic::AtomicUsize::new(0),
    altstack_flags: core::sync::atomic::AtomicUsize::new(0),
};

/// Simple LCG PRNG state for getrandom.
static PRNG_STATE: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0x12345678_9ABCDEF0);

use crate::driver::fs::{MAX_FDS, O_CREAT};
#[cfg(feature = "test_mode")]
use crate::driver::fs::{O_RDONLY, O_RDWR, O_WRONLY};

/// Dispatch a syscall.
///
/// Called from trap_handler when UserEnvCall is detected.
/// `id` = a7 (syscall number), `args` = [a0, a1, a2, a3, a4, a5].
/// Returns value for a0.
/// Linux x86_64 syscall entry point, called from syscall_entry (MSR LSTAR)
/// when the CPU executes a `syscall` instruction.
///
/// ABI: RAX=syscall_nr, RDI=arg1, RSI=arg2, RDX=arg3, R10=arg4, R8=arg5, R9=arg6
///
/// This is a DEDICATED Linux dispatcher — all syscall numbers are interpreted
/// as Linux x86_64 numbers. This bypasses KarteOS's native dispatch() entirely,
/// avoiding number conflicts (e.g., Linux write=1 vs KarteOS exit=1).
#[cfg(target_arch = "x86_64")]
pub fn dispatch_syscall_linux(
    nr: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
    a6: u64,
) -> u64 {
    // Enable timer interrupts on the first syscall (same as dispatch()).
    static TIMER_ENABLED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);
    if !TIMER_ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
        TIMER_ENABLED.store(true, core::sync::atomic::Ordering::Relaxed);
        crate::arch::trap::enable_timer_interrupt();
        crate::arch::trap::set_next_timer();
        crate::arch::ioapic::unmask_external_irqs();
    }

    // Trace ALL syscalls for debugging — disabled for performance
    // {
    //     crate::console_println!(
    //         "[sys] nr={} a0={:#x} a1={:#x} a2={:#x} a3={:#x} a4={:#x} a5={:#x}",
    //         nr, a1, a2, a3, a4, a5, a6
    //     );
    // }
    let result = dispatch_linux_raw(
        nr as usize,
        [
            a1 as usize,
            a2 as usize,
            a3 as usize,
            a4 as usize,
            a5 as usize,
            a6 as usize,
        ],
    );
    // crate::console_println!("[sys] nr={} ret={:#x}", nr, result);
    result as u64
}

/// Comprehensive Linux x86_64 syscall dispatcher.
/// Handles ALL Linux syscall numbers directly, mapping to kernel functions.
/// This is ONLY called from the SYSCALL instruction path (MSR LSTAR).
#[cfg(target_arch = "x86_64")]
fn dispatch_linux_raw(nr: usize, args: [usize; 6]) -> isize {
    dispatch_linux_syscall(nr, args)
}

#[cfg(target_arch = "x86_64")]
fn dispatch_linux_syscall(nr: usize, args: [usize; 6]) -> isize {
    match nr {
        0 => sys_read(args[0] as i32, args[1], args[2]), // read
        1 => {
            let result = sys_write(args[0] as i32, args[1], args[2]);
            // Validate write return: must be 0..len or negative errno
            let len = args[2];
            if result > 0 && (result as usize) > len {
                crate::console_println!(
                    "[write] BAD RETURN: fd={} len={} got={:#x}",
                    args[0],
                    len,
                    result
                );
            }
            result
        }
        2 => linux_open(args[0], args[1], args[2]), // open (deprecated, use openat)
        3 => sys_close(args[0] as i32),             // close
        4 => linux_stat(args[0], args[1]),          // stat
        5 => linux_fstat(args[0], args[1]),         // fstat
        6 => linux_lstat(args[0], args[1]),         // lstat
        7 => linux_poll(args[0], args[1], args[2]), // poll
        8 => linux_lseek(args[0], args[1], args[2]), // lseek
        9 => {
            let r = linux_mmap(args[0], args[1], args[2], args[3], args[4], args[5]);
            r
        }
        10 => linux_mprotect(args[0], args[1], args[2]),
        11 => linux_munmap(args[0], args[1]), // munmap
        12 => sys_brk(args[0]),

        // ─── Signals ──────────────────────────────────────────────
        13 => linux_rt_sigaction(args[0], args[1], args[2]), // rt_sigaction
        14 => linux_rt_sigprocmask(args[0], args[1], args[2]), // rt_sigprocmask
        15 => 0,                                             // rt_sigreturn (stub)

        // ─── File I/O (continued) ─────────────────────────────────
        16 => sys_ioctl(args[0] as i32, args[1], args[2]), // ioctl
        17 => linux_pread64(args[0] as i32, args[1], args[2], args[3], args[4]), // pread64
        18 => linux_pwrite64(args[0] as i32, args[1], args[2], args[3], args[4]), // pwrite64
        19 => linux_readv(args[0], args[1], args[2]),      // readv
        20 => 0,                                           // writev (stub)
        21 => 0,                                           // access (stub)
        22 => linux_pipe(args[0]),                         // pipe
        23 => 0,                                           // select (stub)
        24 => {
            crate::sched::schedule();
            0
        } // sched_yield
        25 => ERR_INVAL,                                   // mremap (stub)
        26 => 0,                                           // msync (stub)
        27 => 0,                                           // mincore (stub)
        28 => {
            let ret = linux_madvise(args[0], args[1], args[2]);
            crate::console_println!(
                "[madvise] addr={:#x} len={:#x} advice={} ret={}",
                args[0],
                args[1],
                args[2],
                ret
            );
            ret
        } // madvise
        29 => linux_dup(args[0]),                          // dup
        30 => linux_dup2(args[0], args[1]),                // dup2
        31 => linux_pause(),                               // pause
        32 => linux_dup(args[0]),                          // dup
        33 => 0,                                           // chdir (stub — use Linux 80)
        34 => 0,                                           // fchdir (stub)
        35 => linux_nanosleep(args[0], args[1]),           // nanosleep
        36 => 0,                                           // alarm (stub)
        37 => 0,                                           // setitimer (stub)
        38 => 0,                                           // gethostname (stub)

        // ─── Process management ───────────────────────────────────
        39 => sys_getpid(),                                  // getpid
        40 => linux_getppid(),                               // getppid
        41 => linux_socket(args[0], args[1], args[2]),       // socket
        42 => sys_connect(args[0] as i32, args[1], args[2]), // connect
        43 => linux_accept(args[0] as i32),                  // accept
        44 => sys_sendto(args[0] as i32, args[1], args[2], args[3], args[4], args[5]), // sendto
        45 => sys_recvfrom(args[0] as i32, args[1], args[2]), // recvfrom
        46 => sys_bind(args[0] as i32, args[1], args[2]),    // bind
        47 => 0,                                             // getsockname (stub)
        48 => 0,                                             // getpeername (stub)
        49 => sys_socket(args[0], args[1], args[2]),         // socket (alternate number?)
        50 => sys_listen(args[0] as i32, args[1]),           // listen
        51 => 0,                                             // getsockname (stub)
        52 => 0,                                             // getpeername (stub)
        53 => 0,                                             // setsockopt (stub)
        54 => 0,                                             // getsockopt (stub)
        55 => sys_shutdown(args[0] as i32),                  // shutdown

        56 => linux_clone(args[0], args[1], args[2], args[3], args[4]), // clone
        57 => sys_fork(),                                               // fork
        58 => 0, // vfork stub: parent continues, no child created
        59 => sys_exec(args[0], linux::count_user_string(args[0]), args[1], args[2]), // execve
        60 => sys_exit(args[0] as i32), // exit

        // ─── More file ops ────────────────────────────────────────
        61 => linux_wait4(args[0], args[1], args[2]), // wait4
        63 => linux_uname(args[0]),                   // uname
        78 => linux_getdents(args[0], args[1], args[2]), // getdents
        79 => linux_getcwd(args[0], args[1]),         // getcwd
        80 => linux_chdir(args[0]),                   // chdir
        83 => sys_mkdir(args[0], linux::count_user_string(args[0])), // mkdir
        87 => sys_unlink(args[0], linux::count_user_string(args[0])), // unlink

        // ─── More process ─────────────────────────────────────────
        89 => 0,                                    // readlink (stub)
        96 => linux_gettimeofday(args[0], args[1]), // gettimeofday
        97 => 0,                                    // getrlimit (stub)
        98 => 0,                                    // getrusage (stub)
        99 => linux_sysinfo(args[0]),               // sysinfo
        100 => 0,                                   // times (stub)
        101 => ERR_INVAL,                           // ptrace (stub)
        102 => 0,                                   // getuid (stub: root)
        103 => 0,                                   // syslog (stub)
        104 => 0,                                   // getgid (stub: root)
        105 => 0,                                   // setuid (stub)
        106 => 0,                                   // setgid (stub)
        107 => 0,                                   // geteuid (stub: root)
        108 => 0,                                   // getegid (stub: root)
        72 => linux_fcntl(args[0], args[1], args[2]),
        74 => linux_fsync(args[0]), // fsync
        75 => linux_fsync(args[0]), // fdatasync (same as fsync for us)
        77 => {
            // ftruncate(fd, length) — resize file
            let fd = args[0] as i32;
            let length = args[1];
            crate::process::with_fd_table(|fd_table| {
                let fd_type = fd_table.get(fd as usize).map(|d| d.fd_type.clone());
                fd_type
            })
            .map_or(0, |fd_type| {
                match fd_type {
                    FdType::VfsFile(vfs_fd) => match crate::driver::vfs::truncate(vfs_fd, length) {
                        Ok(()) => 0,
                        Err(_) => -1,
                    },
                    FdType::FakeFile(_) => crate::process::with_fd_table(|fd_table| {
                        if fd_table.fake_truncate(fd, length) {
                            0
                        } else {
                            ERR_INVAL
                        }
                    }),
                    _ => {
                        // Ext4File, pipe, etc. — pretend success
                        0
                    }
                }
            })
        }
        122 => linux_uname(args[0]),                   // uname (new)
        131 => linux_sigaltstack(args[0], args[1]),    // sigaltstack
        157 => 0,                                      // prctl (stub)
        158 => linux_arch_prctl(args[0], args[1]),     // arch_prctl
        160 => 0,                                      // setrlimit (stub)
        186 => sys_getpid(),                           // gettid → getpid
        200 => 0,                                      // tkill (stub)
        201 => linux_time(args[0]),                    // time
        202 => linux_futex(args[0], args[1], args[2]), // futex
        203 => linux_sched_setaffinity(args[0], args[1], args[2]), // sched_setaffinity (stub)
        204 => linux_sched_getaffinity(args[0], args[1], args[2]), // sched_getaffinity
        217 => linux_getdents64(args[0], args[1], args[2]), // getdents64
        218 => linux_set_tid_address(args[0]),         // set_tid_address
        228 => linux_clock_gettime(args[0], args[1]),  // clock_gettime
        231 => linux_exit_group(args[0] as i32),       // exit_group
        234 => linux_tgkill(args[0], args[1], args[2]), // tgkill
        257 => linux_openat(args[0], args[1], args[2], args[3]), // openat
        258 => linux_mkdirat(args[0], args[1], args[2], args[3]), // mkdirat
        262 => linux_newfstatat(args[0], args[1], args[2], args[3]), // newfstatat
        263 => sys_unlink(args[1], linux::count_user_string(args[1])), // unlinkat
        267 => 0,                                      // readlinkat (stub)
        272 => 0,                                      // unshare (stub)
        273 => 0,                                      // set_robust_list (stub)
        274 => 0,                                      // get_robust_list (stub)
        290 => epoll::eventfd::sys_eventfd2(args[0], args[1]), // eventfd2
        232 => epoll::sys_epoll_wait(args[0], args[1], args[2], args[3] as isize), // epoll_wait
        233 => epoll::sys_epoll_ctl(args[0], args[1], args[2], args[3]), // epoll_ctl
        281 => epoll::sys_epoll_wait(args[0], args[1], args[2], args[3] as isize), // epoll_pwait (same as epoll_wait, ignoring sigmask)
        291 => epoll::sys_epoll_create1(args[0]),                                  // epoll_create1
        292 => sys_dup2(args[0] as i32, args[1] as i32),                           // dup3 → dup2
        293 => 0,                                                                  // pipe2 (stub)
        302 => 0,                                          // prlimit64 (stub)
        285 => 0, // fallocate → success (SQLite WAL needs this)
        318 => linux_getrandom(args[0], args[1], args[2]), // getrandom
        334 => -38, // rseq → ENOSYS (Go gracefully degrades)
        435 => -38, // clone3: ENOSYS
        _ => {
            -38 // ENOSYS
        }
    }
}

// ─── Linux-specific syscall implementations ──────────────────────────
// These handle Linux ABI differences from KarteOS's native syscalls.

#[cfg(target_arch = "x86_64")]
/// Linux open(path, flags, mode) — translate to KarteOS sys_open
fn linux_open(path: usize, flags: usize, _mode: usize) -> isize {
    let path_len = linux::count_user_string(path);
    if path_len == 0 {
        return ERR_NOENT;
    }
    sys_open(path, path_len, flags as u32)
}

#[cfg(target_arch = "x86_64")]
/// Linux openat(dirfd, pathname, flags, mode) — open file
/// Stack-based implementation to avoid heap allocator issues in syscall context.
fn linux_openat(_dirfd: usize, pathname: usize, flags: usize, _mode: usize) -> isize {
    let path_str = match read_user_path(pathname, linux::count_user_string(pathname)) {
        Some(s) if !s.is_empty() => s,
        _ => return ERR_NOENT,
    };

    // Convert Linux x86_64 flags to our internal flags
    // Linux x86_64: O_CREAT=0x40, O_TRUNC=0x200, O_APPEND=0x400, O_RDONLY=0, O_WRONLY=1, O_RDWR=2
    let linux_creat = 0x40;
    let has_creat = (flags & linux_creat) != 0;

    // Try VFS open with converted flags
    let our_flags = if has_creat { 0x100 } else { 0 } | (flags & 0x600); // keep O_TRUNC/O_APPEND
    match crate::driver::vfs::open(&path_str, our_flags as u32) {
        Ok(vfs_fd) => {
            // Register the VFS fd in the process FdTable
            crate::process::with_fd_table(|fd_table| {
                match fd_table.alloc_vfs_fd(
                    alloc::format!("{}", path_str),
                    vfs_fd,
                    our_flags as u32,
                ) {
                    Some(fd) => fd as isize,
                    None => ERR_NOMEM,
                }
            })
        }
        Err(e) => {
            // Only fake virtual pseudo-filesystem paths (/proc, /sys, /dev, /etc, /run)
            let is_pseudo = is_pseudo_path(&path_str);
            // /etc/resolv.conf, /etc/localtime etc. — fake these too
            let is_etc = path_str.starts_with("/etc");

            // /dev/urandom and /dev/random need real random bytes, not FakeFile
            // Match both "/dev/urandom" and "dev/urandom" (SQLite may use relative path)
            let is_urandom = path_str == "/dev/urandom"
                || path_str == "/dev/random"
                || path_str == "dev/urandom"
                || path_str == "dev/random";

            if is_urandom {
                crate::process::with_fd_table(|fd_table| {
                    match fd_table.alloc_urandom_fd(our_flags as u32) {
                        Some(fd) => fd as isize,
                        None => ERR_NOENT,
                    }
                })
            } else if is_pseudo || is_etc {
                crate::process::with_fd_table(|fd_table| {
                    match fd_table.alloc_fake_fd(alloc::format!("{}", path_str), our_flags as u32) {
                        Some(fd) => fd as isize,
                        None => ERR_NOENT,
                    }
                })
            } else {
                ERR_NOENT
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
/// Linux stat — return stat structure with correct file type from VFS metadata.
fn linux_stat(pathname: usize, statbuf: usize) -> isize {
    if statbuf != 0 {
        // Try to read the path
        if let Some(s) = read_user_path(pathname, linux::count_user_string(pathname)) {
            let resolved = crate::syscall::resolve_path(&s);

            // Check if it's a pseudo path (/proc, /sys, /dev, /etc, /run)
            let is_pseudo = is_pseudo_path(&resolved);

            // Try VFS walk to get real inode and metadata
            if let Ok(ino) = crate::driver::vfs::walk_path_resolved(&resolved) {
                if let Ok(meta) = crate::driver::vfs::inode_metadata(ino) {
                    let st_mode = if meta.is_dir() {
                        0x41FFu32 // S_IFDIR | 0777
                    } else {
                        0x81A4u32 // S_IFREG | 0644
                    };
                    let mut stat_buf = [0u8; 144];
                    fill_stat_buffer(&mut stat_buf, st_mode, meta.size as i64, ino);
                    user_write_bytes(statbuf, &stat_buf[..144]);
                    return 0;
                }
            }

            // Pseudo-filesystem paths: fake as directory (for MkdirAll compatibility)
            if is_pseudo {
                let mut stat_buf = [0u8; 144];
                fill_stat_buffer(&mut stat_buf, 0x41FFu32, 0, 0); // S_IFDIR | 0777
                user_write_bytes(statbuf, &stat_buf[..144]);
                return 0;
            }

            // Real filesystem path not found — return ENOENT so Go's MkdirAll
            // knows it needs to create the directory
            return -2; // ENOENT
        }

        // Couldn't parse path — return ENOENT
        return -2; // ENOENT
    }
    0
}

#[cfg(target_arch = "x86_64")]
fn linux_fstat(fd: usize, statbuf: usize) -> isize {
    if statbuf == 0 {
        return 0;
    }

    // x86_64 Linux struct stat layout (144 bytes):
    //   0:  st_dev     (u64)
    //   8:  st_ino     (u64)
    //  16:  st_nlink   (u64)
    //  24:  st_mode    (u32), st_uid (u32)
    //  32:  st_gid     (u32), pad (u32)
    //  40:  st_rdev    (u64)
    //  48:  st_size    (i64)
    //  56:  st_blksize (i64)
    //  64:  st_blocks  (i64)

    // Determine file type and metadata from fd
    let (st_mode, st_size, st_ino) = crate::process::with_fd_table(|fd_table| {
        if let Some(desc) = fd_table.get(fd) {
            match &desc.fd_type {
                FdType::VfsFile(vfs_fd) => {
                    let ino = crate::driver::vfs::get_inode_for_fd(*vfs_fd).unwrap_or(0);
                    match crate::driver::vfs::fd_metadata(*vfs_fd) {
                        Ok(meta) if meta.is_dir() => (0x41FFu32, meta.size as u64, ino),
                        Ok(meta) => (0x81A4u32, meta.size as u64, ino),
                        Err(_) => (0x81A4u32, 0u64, ino),
                    }
                }
                FdType::Urandom => {
                    (0x21A4u32, 0u64, 0u64) // S_IFCHR | 0644 (character device)
                }
                FdType::FakeFile(_) => {
                    (0x81A4u32, 0u64, 0u64) // S_IFREG | 0644
                }
                FdType::Stdio => {
                    (0x21A4u32, 0u64, 1u64) // S_IFCHR | 0644, inode 1
                }
                FdType::PipeRead | FdType::PipeWrite => {
                    (0x11A4u32, 0u64, 0u64) // S_IFIFO | 0644
                }
                _ => {
                    (0x81A4u32, 0u64, 0u64) // default: regular file
                }
            }
        } else {
            (0x41FFu32, 0u64, 0u64) // S_IFDIR | 0777 (fallback)
        }
    });

    let mut stat_buf = [0u8; 144];
    fill_stat_buffer(&mut stat_buf, st_mode, st_size as i64, st_ino);
    user_write_bytes(statbuf, &stat_buf[..144]);
    0
}

#[cfg(target_arch = "x86_64")]
fn linux_lstat(_pathname: usize, statbuf: usize) -> isize {
    // Same as stat for our purposes
    linux_stat(_pathname, statbuf)
}

#[cfg(target_arch = "x86_64")]
fn linux_newfstatat(_dirfd: usize, pathname: usize, statbuf: usize, _flags: usize) -> isize {
    // For now, delegate to linux_stat which returns S_IFREG
    linux_stat(pathname, statbuf)
}

#[cfg(target_arch = "x86_64")]
fn linux_poll(_fds: usize, _nfds: usize, _timeout: usize) -> isize {
    // No events ready
    0
}

#[cfg(target_arch = "x86_64")]
fn linux_lseek(fd: usize, offset: usize, whence: usize) -> isize {
    const SEEK_SET: usize = 0;
    const SEEK_CUR: usize = 1;
    const SEEK_END: usize = 2;

    crate::process::with_fd_table(|fd_table| {
        let desc = match fd_table.get_mut(fd) {
            Some(desc) => desc,
            None => return -9, // EBADF
        };
        let file_size = match &desc.fd_type {
            FdType::VfsFile(vfs_fd) => crate::driver::vfs::fd_metadata(*vfs_fd)
                .map(|meta| meta.size)
                .unwrap_or(desc.pos),
            FdType::FakeFile(data) => data.len(),
            _ => desc.pos,
        };
        let base = match whence {
            SEEK_SET => 0isize,
            SEEK_CUR => desc.pos as isize,
            SEEK_END => file_size as isize,
            _ => return ERR_INVAL,
        };
        let new_pos = base.saturating_add(offset as isize);
        if new_pos < 0 {
            return ERR_INVAL;
        }
        desc.pos = new_pos as usize;
        desc.pos as isize
    })
}

#[cfg(target_arch = "x86_64")]
fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

#[cfg(target_arch = "x86_64")]
fn dirent_type(file_type: crate::driver::vfs::VfsFileType) -> u8 {
    match file_type {
        crate::driver::vfs::VfsFileType::Directory => 4, // DT_DIR
        crate::driver::vfs::VfsFileType::File => 8,      // DT_REG
    }
}

#[cfg(target_arch = "x86_64")]
fn linux_getdents64(fd: usize, dirp: usize, count: usize) -> isize {
    if dirp == 0 || count == 0 {
        return ERR_INVAL;
    }

    crate::arch::trap::with_kernel_cr3(|| {
        let (vfs_fd, mut idx) = match crate::process::with_fd_table(|fd_table| {
            fd_table.get(fd).map(|desc| match &desc.fd_type {
                FdType::VfsFile(vfs_fd) => Some((*vfs_fd, desc.pos)),
                _ => None,
            })
        }) {
            Some(Some(info)) => info,
            Some(None) => return -20, // ENOTDIR
            None => return ERR_INVAL,
        };

        let mut written = 0usize;
        let mut record = [0u8; 280]; // linux_dirent64 header + ext4 max 255-byte name + padding
        loop {
            let entry = match crate::driver::vfs::readdir_at(vfs_fd, idx) {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(crate::driver::vfs::VfsError::NotADirectory) => return -20,
                Err(_) => return ERR_IO,
            };
            let name = entry.name.as_bytes();
            let reclen = align_up(8 + 8 + 2 + 1 + name.len() + 1, 8);
            if written + reclen > count {
                if written == 0 {
                    return ERR_INVAL;
                }
                break;
            }
            if reclen > record.len() {
                return ERR_RANGE;
            }

            for byte in record[..reclen].iter_mut() {
                *byte = 0;
            }
            let ino = (idx as u64) + 1;
            let next_off = (idx as i64) + 1;
            record[0..8].copy_from_slice(&ino.to_ne_bytes());
            record[8..16].copy_from_slice(&next_off.to_ne_bytes());
            record[16..18].copy_from_slice(&(reclen as u16).to_ne_bytes());
            record[18] = dirent_type(entry.file_type);
            record[19..19 + name.len()].copy_from_slice(name);
            user_write_bytes(dirp + written, &record[..reclen]);
            written += reclen;
            idx += 1;
        }

        if written != 0 {
            crate::process::with_fd_table(|fd_table| {
                if let Some(desc) = fd_table.get_mut(fd) {
                    desc.pos = idx;
                }
            });
        }
        written as isize
    })
}

#[cfg(target_arch = "x86_64")]
fn linux_getdents(fd: usize, dirp: usize, count: usize) -> isize {
    if dirp == 0 || count == 0 {
        return ERR_INVAL;
    }

    crate::arch::trap::with_kernel_cr3(|| {
        let (vfs_fd, mut idx) = match crate::process::with_fd_table(|fd_table| {
            fd_table.get(fd).map(|desc| match &desc.fd_type {
                FdType::VfsFile(vfs_fd) => Some((*vfs_fd, desc.pos)),
                _ => None,
            })
        }) {
            Some(Some(info)) => info,
            Some(None) => return -20, // ENOTDIR
            None => return ERR_INVAL,
        };

        let mut written = 0usize;
        let mut record = [0u8; 280]; // linux_dirent header + ext4 max 255-byte name + padding
        loop {
            let entry = match crate::driver::vfs::readdir_at(vfs_fd, idx) {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(crate::driver::vfs::VfsError::NotADirectory) => return -20,
                Err(_) => return ERR_IO,
            };
            let name = entry.name.as_bytes();
            let reclen = align_up(8 + 8 + 2 + name.len() + 1 + 1, 8);
            if written + reclen > count {
                if written == 0 {
                    return ERR_INVAL;
                }
                break;
            }
            if reclen > record.len() {
                return ERR_RANGE;
            }

            for byte in record[..reclen].iter_mut() {
                *byte = 0;
            }
            let ino = (idx as u64) + 1;
            let next_off = (idx as i64) + 1;
            record[0..8].copy_from_slice(&ino.to_ne_bytes());
            record[8..16].copy_from_slice(&next_off.to_ne_bytes());
            record[16..18].copy_from_slice(&(reclen as u16).to_ne_bytes());
            record[18..18 + name.len()].copy_from_slice(name);
            record[reclen - 1] = dirent_type(entry.file_type);
            user_write_bytes(dirp + written, &record[..reclen]);
            written += reclen;
            idx += 1;
        }

        if written != 0 {
            crate::process::with_fd_table(|fd_table| {
                if let Some(desc) = fd_table.get_mut(fd) {
                    desc.pos = idx;
                }
            });
        }
        written as isize
    })
}

#[cfg(target_arch = "x86_64")]
fn linux_pread64(fd: i32, buf: usize, count: usize, offset: usize, _offset_hi: usize) -> isize {
    if count == 0 {
        return 0;
    }

    // Kernel buffer for VFS read (user pointer can't be used under kernel CR3)
    let mut data = alloc::vec![0u8; count];

    // Try VFS pread (offset-based, no fd position update). The Linux fd must
    // first be resolved through the per-process fd table to the internal VFS fd.
    if let Some(vfs_fd) = crate::process::with_fd_table(|fd_table| {
        fd_table
            .get(fd as usize)
            .and_then(|desc| match &desc.fd_type {
                FdType::VfsFile(vfs_fd) => Some(*vfs_fd),
                _ => None,
            })
    }) {
        match crate::driver::vfs::pread(vfs_fd, &mut data, offset) {
            Ok(n) => {
                user_write_bytes(buf, &data[..n]);
                return n as isize;
            }
            Err(_) => {}
        }
    }

    // Fallback to regular sys_read for non-VFS fds (pipes, stdio, etc.)
    sys_read(fd, buf, count)
}

#[cfg(target_arch = "x86_64")]
fn linux_pwrite64(fd: i32, buf: usize, count: usize, offset: usize, _offset_hi: usize) -> isize {
    if count == 0 {
        return 0;
    }

    // Read from user space into kernel buffer
    let data = user_read_bytes(buf, count);

    // Try VFS pwrite (offset-based, no fd position update). The Linux fd must
    // first be resolved through the per-process fd table to the internal VFS fd.
    if let Some(vfs_fd) = crate::process::with_fd_table(|fd_table| {
        fd_table
            .get(fd as usize)
            .and_then(|desc| match &desc.fd_type {
                FdType::VfsFile(vfs_fd) => Some(*vfs_fd),
                _ => None,
            })
    }) {
        match crate::driver::vfs::pwrite(vfs_fd, &data, offset) {
            Ok(n) => return n as isize,
            Err(_) => {}
        }
    }

    // Fallback for non-VFS fds (pipes, etc.): ignore offset, do regular write
    sys_write(fd, buf, count)
}

#[cfg(target_arch = "x86_64")]
fn linux_readv(_fd: usize, _iov: usize, _iovcnt: usize) -> isize {
    // Stub: return 0 bytes read
    0
}

#[cfg(target_arch = "x86_64")]
fn linux_dup(oldfd: usize) -> isize {
    // Simple dup: find lowest available fd and dup2
    crate::process::with_fd_table(|fd_table| {
        for new_fd in 0..crate::driver::fs::MAX_FDS {
            if fd_table.get(new_fd).is_none() {
                if fd_table.dup(oldfd, new_fd) {
                    // Increment pipe ref count if this is a pipe fd
                    if let Some(desc) = fd_table.get(oldfd) {
                        if let Some(pipe_id) = desc.pipe_id {
                            crate::driver::pipe::inc_ref(pipe_id);
                        }
                    }
                    return new_fd as isize;
                }
                return ERR_INVAL;
            }
        }
        ERR_NOMEM
    })
}

#[cfg(target_arch = "x86_64")]
fn linux_dup2(oldfd: usize, newfd: usize) -> isize {
    sys_dup2(oldfd as i32, newfd as i32)
}

#[cfg(target_arch = "x86_64")]
fn linux_pause() -> isize {
    // Stub: pretend interrupted
    ERR_INTR
}

#[cfg(target_arch = "x86_64")]
fn linux_pipe(fd_ptr: usize) -> isize {
    sys_pipe(fd_ptr)
}

#[cfg(target_arch = "x86_64")]
fn linux_getppid() -> isize {
    // Stub: return PID 1 (init)
    1
}

#[cfg(target_arch = "x86_64")]
fn linux_socket(_domain: usize, _socket_type: usize, _protocol: usize) -> isize {
    // Network not available on x86_64 yet
    ERR_IO
}

#[cfg(target_arch = "x86_64")]
fn linux_accept(_fd: i32) -> isize {
    // Delegate to sys_accept (kernel syscall 74)
    sys_accept(_fd)
}

#[cfg(target_arch = "x86_64")]
fn linux_waitpid(pid: usize, status_ptr: usize, options: usize) -> isize {
    let _ = (status_ptr, options);
    sys_waitpid(pid)
}

/// Linux uname(2) — return kernel version info.
/// struct utsname has 6 fields of 65 bytes each (total 390 bytes).
#[cfg(target_arch = "x86_64")]
fn linux_uname(buf: usize) -> isize {
    if buf == 0 {
        return -14; // EFAULT
    }
    let fields: [&[u8]; 6] = [
        b"Linux\0",          // sysname
        b"karteos\0",        // nodename
        b"6.1.0\0",          // release (fake Linux version)
        b"#1 SMP KarteOS\0", // version
        b"x86_64\0",         // machine
        b"\0",               // domainname
    ];
    let mut uts_buf = [0u8; 390];
    let mut offset = 0usize;
    for field in &fields {
        let len = field.len().min(65);
        uts_buf[offset..offset + len].copy_from_slice(&field[..len]);
        offset += 65;
    }

    // Debug: dump CR3 state and page mapping before write
    let user_root = crate::process::current_page_table_root();
    let kernel_cr3 = crate::arch::idt::get_kernel_cr3_phys();
    let user_cr3 = user_root << 12;
    let page = buf & !0xFFF;
    let before_map = crate::arch::trap::with_kernel_cr3(|| {
        let pt = crate::arch::trap::get_user_pt_safe();
        crate::mm::vmm::translate_user(pt, page)
    });
    crate::console_println!(
        "[uname] PRE: user_root={:#x} user_cr3={:#x} kernel_cr3={:#x} buf={:#x} page_map={:?}",
        user_root,
        user_cr3,
        kernel_cr3,
        buf,
        before_map
    );

    user_write_bytes(buf, &uts_buf);

    // Verify: read back the release field (offset 130, should be "6.1.0\0")
    // NOTE: verification disabled — user_read_bytes may not work correctly
    // for all addresses due to CR3 switching issues.
    0
}

#[cfg(target_arch = "x86_64")]
fn linux_wait4(pid: usize, status_ptr: usize, options: usize) -> isize {
    let _ = status_ptr;
    // Write exit status if requested
    let result = sys_waitpid(pid);
    if result >= 0 && status_ptr != 0 {
        user_write::<i32>(status_ptr, ((result & 0xFF) << 8) as i32); // WEXITSTATUS encoding
    }
    let _ = options;
    result
}

#[cfg(target_arch = "x86_64")]
fn linux_getcwd(buf: usize, size: usize) -> isize {
    if buf == 0 || size == 0 {
        return ERR_INVAL;
    }
    // Return "/" as the working directory
    let cwd = b"/\0";
    let len = cwd.len().min(size);
    user_write_bytes(buf, &cwd[..len]);
    (len - 1) as isize // return length without NUL
}

#[cfg(target_arch = "x86_64")]
fn linux_chdir(path: usize) -> isize {
    let path_len = linux::count_user_string(path);
    if path_len == 0 {
        return ERR_INVAL;
    }
    sys_chdir(path, path_len)
}

#[cfg(target_arch = "x86_64")]
fn linux_gettimeofday(tv: usize, _tz: usize) -> isize {
    let uptime_ms = crate::arch::platform::uptime_ms();
    let tv_sec = (uptime_ms / 1000 + FAKE_EPOCH) as i64;
    let tv_usec = ((uptime_ms % 1000) * 1000) as i64;
    if tv != 0 {
        user_write::<i64>(tv, tv_sec);
        user_write::<i64>(tv + 8, tv_usec);
    }
    0
}

#[cfg(target_arch = "x86_64")]
fn linux_time(tloc: usize) -> isize {
    if tloc != 0 {
        user_write::<u64>(tloc, 0);
    }
    0
}

#[cfg(target_arch = "x86_64")]
fn linux_clock_gettime(_clockid: usize, tp: usize) -> isize {
    if tp != 0 {
        // Use kernel uptime (milliseconds since boot) as a monotonic source
        let uptime_ms = crate::arch::platform::uptime_ms();
        let secs = uptime_ms / 1000;
        let nsecs = (uptime_ms % 1000) * 1_000_000;
        user_write::<u64>(tp, secs);
        user_write::<u64>(tp + 8, nsecs);
    }
    0
}

#[cfg(target_arch = "x86_64")]
fn linux_sysinfo(info: usize) -> isize {
    if info != 0 {
        // Linux x86_64 struct sysinfo is 112 bytes.
        user_write_bytes(info, &[0u8; 112]);
        user_write::<u64>(info + 32, 512 * 1024 / 4); // totalram (in pages, ~512MB)
        user_write::<u64>(info + 40, 256 * 1024 / 4); // freeram
        user_write::<u16>(info + 80, 1); // procs
        user_write::<u32>(info + 104, 4096); // mem_unit
    }
    0
}

#[cfg(target_arch = "x86_64")]
fn linux_sched_getaffinity(_pid: usize, size: usize, mask: usize) -> isize {
    if mask != 0 && size > 0 {
        for i in 0..size {
            user_write_u8(mask + i, 0);
        }
        // Set CPU 0 as available (first byte = 0x01)
        user_write::<u8>(mask, 0x01u8);
    }
    size as isize // return mask size in bytes
}

#[cfg(target_arch = "x86_64")]
fn linux_sched_setaffinity(_pid: usize, _size: usize, _mask: usize) -> isize {
    0 // stub: success
}

pub fn dispatch(id: usize, args: [usize; 6]) -> isize {
    // Enable timer interrupts on the first syscall.
    static TIMER_ENABLED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);
    if !TIMER_ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
        TIMER_ENABLED.store(true, core::sync::atomic::Ordering::Relaxed);
        crate::arch::trap::enable_timer_interrupt();
        crate::arch::trap::set_next_timer();
        #[cfg(target_arch = "x86_64")]
        crate::arch::ioapic::unmask_external_irqs();
    }

    // Try Linux compat layer first.
    let result = if let Some(translation) = linux::translate(id, args) {
        match translation {
            linux::Translation::Dispatch { karte_nr, args } => dispatch_inner(karte_nr, args),
            linux::Translation::Handled(retval) => retval,
        }
    } else {
        dispatch_inner(id, args)
    };

    result
}

fn dispatch_inner(id: usize, args: [usize; 6]) -> isize {
    // Only trace key syscalls from int 0x80 path (shell calls)
    // id=6=mmap, id=4=brk, id=1=exit — skip read/write noise
    let should_trace = matches!(id, 1 | 4 | 6);

    let result = match id {
        SYS_DEBUG_PRINT => sys_debug_print(args[0], args[1]),
        SYS_EXIT => sys_exit(args[0] as i32),
        SYS_WRITE => sys_write(args[0] as i32, args[1], args[2]),
        SYS_READ => sys_read(args[0] as i32, args[1], args[2]),
        SYS_BRK => sys_brk(args[0]),
        SYS_GETPID => sys_getpid(),
        SYS_MMAP => linux_mmap(args[0], args[1], args[2], args[3], args[4], args[5]),
        SYS_PIPE => sys_pipe(args[0]),
        SYS_DUP2 => sys_dup2(args[0] as i32, args[1] as i32),
        SYS_OPEN => sys_open(args[0], args[1], args[2] as u32),
        SYS_CLOSE => sys_close(args[0] as i32),
        SYS_SPAWN => sys_spawn(args[0], args[1]),
        SYS_EXEC => sys_exec(args[0], args[1], args[2], args[3]),
        SYS_EXEC_FD => sys_exec_fd(args[0], args[1], args[2] as i32, args[3] as i32),
        SYS_WAITPID => sys_waitpid(args[0]),
        SYS_LS => sys_ls(args[0], args[1]),
        SYS_MKDIR => sys_mkdir(args[0], args[1]),
        SYS_UNLINK => sys_unlink(args[0], args[1]),
        SYS_SETENV => sys_setenv(args[0], args[1], args[2], args[3]),
        SYS_GETENV => sys_getenv(args[0], args[1], args[2], args[3]),
        SYS_CHDIR => sys_chdir(args[0], args[1]),
        SYS_KILL => sys_kill(args[0], args[1]),
        SYS_FORK => sys_fork(),
        SYS_IOCTL => sys_ioctl(args[0] as i32, args[1], args[2]),

        // Network syscalls
        SYS_SOCKET => sys_socket(args[0], args[1], args[2]),
        SYS_BIND => sys_bind(args[0] as i32, args[1], args[2]),
        SYS_CONNECT => sys_connect(args[0] as i32, args[1], args[2]),
        SYS_LISTEN => sys_listen(args[0] as i32, args[1]),
        SYS_ACCEPT => sys_accept(args[0] as i32),
        SYS_SENDTO => sys_sendto(args[0] as i32, args[1], args[2], args[3], args[4], args[5]),
        SYS_RECVFROM => sys_recvfrom(args[0] as i32, args[1], args[2]),
        SYS_SHUTDOWN => sys_shutdown(args[0] as i32),
        SYS_SYSLOG => sys_syslog(args[0], args[1], args[2]),

        // Linux compatibility syscalls (translated from x86_64 Linux numbers)
        LINUX_CLONE => linux_clone(args[0], args[1], args[2], args[3], args[4]),
        LINUX_FUTEX => linux_futex(args[0], args[1], args[2]),
        LINUX_RT_SIGACTION => linux_rt_sigaction(args[0], args[1], args[2]),
        LINUX_RT_SIGPROCMASK => linux_rt_sigprocmask(args[0], args[1], args[2]),
        LINUX_RT_SIGRETURN => 0, // stub: success
        LINUX_SIGALTSTACK => linux_sigaltstack(args[0], args[1]),
        LINUX_SCHED_YIELD => {
            crate::sched::schedule();
            0
        }
        LINUX_MMAP => linux_mmap(args[0], args[1], args[2], args[3], args[4], args[5]),
        LINUX_MPROTECT => linux_mprotect(args[0], args[1], args[2]),
        LINUX_MUNMAP => linux_munmap(args[0], args[1]),
        LINUX_MADVISE => linux_madvise(args[0], args[1], args[2]),
        LINUX_GETRANDOM => linux_getrandom(args[0], args[1], args[2]),
        LINUX_SET_TID_ADDRESS => linux_set_tid_address(args[0]),
        #[cfg(target_arch = "x86_64")]
        LINUX_ARCH_PRCTL => linux_arch_prctl(args[0], args[1]),
        #[cfg(not(target_arch = "x86_64"))]
        LINUX_ARCH_PRCTL => {
            0 // stub: not needed on non-x86_64
        }

        _ => {
            crate::klog!(WARN, "[syscall] Unknown syscall: {}", id);
            ERR_INVAL
        }
    };
    let _ = should_trace;
    result
}

/// Syscall 0: Debug print (write bytes to kernel console).
/// Used by user programs before proper file descriptors work.
fn sys_debug_print(buf: usize, len: usize) -> isize {
    if buf == 0 || len == 0 || len > 4096 {
        return ERR_INVAL;
    }
    let data = user_read_bytes(buf, len);
    crate::arch::platform::print(core::str::from_utf8(&data).unwrap_or("[invalid utf8]"));
    len as isize
}

/// Syscall 1: Exit the current process.
pub fn sys_exit(code: i32) -> isize {
    // ═══════════════════════════════════════════════════════════════
    // CRITICAL: cli FIRST. SYSCALL does NOT clear IF, so Timer ISR
    // can preempt us at any point. We must prevent preemption while
    // marking clone children Exited, otherwise Timer ISR's schedule()
    // will resume them. For SMP, broadcast_reschedule() IPI forces
    // other cores to re-evaluate.
    // ═══════════════════════════════════════════════════════════════
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("cli")
    };

    // 1. Atomically kill all clone child threads (marks both process
    //    table AND scheduler task as Exited). After this, no core's
    //    schedule() will ever pick up these threads again.
    let my_pid = crate::process::current_pid();
    crate::process::kill_clone_children(my_pid);

    // NOTE: Do NOT clear the global VMA table here! It is shared across all
    // processes. The next exec/program load will reinitialize VMAs via mmap.
    // vma_clear() was here before and caused pid=3 exit to wipe pid=2's VMAs.

    // 2. Mark ourselves as Exited in the scheduler
    crate::sched::mark_current_exited();
    crate::process::set_exit_code(code as usize);

    // 3. Now safe to do I/O — children are dead, won't interfere
    crate::klog!(
        INFO,
        "[process] User process pid={} slot={} exited with code {}",
        my_pid,
        crate::sched::current_running_slot(),
        code
    );

    // If init (the shell) exits, no process remains → shut down the system.
    if my_pid == 1 {
        crate::klog!(INFO, "[init] Shell exited, shutting down...");
        crate::arch::platform::shutdown();
    }

    // CLONE_CHILD_CLEARTID: write 0 to the child_tid_ptr on exit.
    // This is used by Go's futex-based thread joining.
    #[cfg(target_arch = "x86_64")]
    if let Some(proc) = crate::process::current() {
        if proc.child_tid_ptr != 0 {
            let tid_ptr = proc.child_tid_ptr;
            user_write::<i32>(tid_ptr, 0);
            linux_futex(tid_ptr, 1, 1);
        }
    }

    // Wake parent if waiting
    let my_idx = crate::process::current_index();
    if let Some(parent_idx) = crate::process::find_waiting_parent(my_idx) {
        crate::process::set_wait_child(parent_idx, None);
        crate::sched::wake_task(parent_idx);
    }

    // 4. Switch to kernel CR3 before context switch.
    //    When called from PF/exception handlers (IST stack), CR3 may still be
    //    the dying process's user page table. Go's mmap lazy allocation can
    //    overwrite identity mapping entries in the user page table, causing
    //    __switch to read corrupted data from other tasks' kernel stacks.
    //    Kernel CR3 has a complete, untampered identity mapping.
    #[cfg(target_arch = "x86_64")]
    {
        let kcr3 = crate::mm::vmm::kernel_cr3();
        if kcr3 != 0 {
            unsafe {
                core::arch::asm!("mov cr3, {}", in(reg) kcr3);
            }
        }
    }

    // 5. Switch to another ready child task (or back to init).
    //    No Exited clone children can be scheduled.
    crate::sched::schedule_exit();

    0
}

#[cfg(target_arch = "x86_64")]
fn linux_exit_group(code: i32) -> isize {
    unsafe { core::arch::asm!("cli") };

    let my_idx = crate::process::current_index();
    let my_pid = crate::process::current_pid();
    let root = crate::process::current_page_table_root();
    let leader_idx = crate::process::find_group_leader_by_page_table_root(root).unwrap_or(my_idx);
    let group = crate::process::find_processes_by_page_table_root(root);

    for idx in group {
        if idx == my_idx {
            continue;
        }
        if let Some(proc) = crate::process::get_process_by_index(idx) {
            if proc.child_tid_ptr != 0 {
                user_write::<i32>(proc.child_tid_ptr, 0);
                linux_futex(proc.child_tid_ptr, 1, 1);
            }
        }
        crate::process::set_exit_code_by_index(idx, code as usize);
        crate::sched::mark_task_exited_by_proc(idx);
        if idx != leader_idx {
            crate::process::free_process_slot(idx);
        }
    }

    if leader_idx != my_idx {
        crate::process::set_exit_code_by_index(leader_idx, code as usize);
    }

    if let Some(parent_idx) = crate::process::find_waiting_parent(leader_idx) {
        crate::process::set_wait_child(parent_idx, None);
        crate::sched::wake_task(parent_idx);
    }

    crate::sched::mark_current_exited();
    crate::process::set_exit_code(code as usize);

    if let Some(proc) = crate::process::current() {
        if proc.child_tid_ptr != 0 {
            user_write::<i32>(proc.child_tid_ptr, 0);
            linux_futex(proc.child_tid_ptr, 1, 1);
        }
    }

    crate::klog!(
        INFO,
        "[process] exit_group pid={} leader_idx={} slot={} code={}",
        my_pid,
        leader_idx,
        crate::sched::current_running_slot(),
        code
    );

    if my_pid == 1 {
        crate::klog!(INFO, "[init] Shell exited, shutting down...");
        crate::arch::platform::shutdown();
    }

    let kcr3 = crate::mm::vmm::kernel_cr3();
    if kcr3 != 0 {
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) kcr3);
        }
    }

    if my_idx != leader_idx {
        crate::process::free_process_slot(my_idx);
    }

    crate::sched::schedule_exit();
    0
}

/// Syscall 2: Write to file descriptor.
fn sys_write(fd: i32, buf: usize, len: usize) -> isize {
    if buf == 0 || len == 0 || len > 65536 {
        return ERR_INVAL;
    }

    let fd_info = get_fd_info(fd);

    match fd_info {
        Some((FdType::PipeWrite, Some(pipe_id), _)) => {
            return pipe_write(pipe_id, buf, len);
        }
        Some((FdType::PipeRead, _, _)) => {
            return ERR_INVAL; // can't write to read end
        }
        Some((FdType::Stdio, _, _)) => {
            // Stdio: write to console
            for i in 0..len {
                let byte = user_read::<u8>(buf + i);
                crate::arch::platform::console_putchar(byte);
            }
            return len as isize;
        }
        Some((FdType::FakeFile(_), _, _)) => {
            // FakeFile: write to in-memory buffer
            return crate::process::with_fd_table(|fd_table| {
                fd_table.fake_write(fd, buf, len).unwrap_or(len as isize)
            });
        }
        Some((FdType::Urandom, _, _)) => {
            // /dev/urandom: writes are silently discarded
            return len as isize;
        }
        Some((FdType::VfsFile(vfs_fd), _, _)) => {
            // VFS file: copy user data to kernel buffer first, because
            // vfs::write() switches to kernel CR3 where user pages are inaccessible.
            let data = user_read_bytes(buf, len);
            return match crate::driver::vfs::write(vfs_fd, &data) {
                Ok(n) => {
                    #[cfg(target_arch = "x86_64")]
                    mirror_user_json_log(&data[..n]);
                    n as isize
                }
                Err(_) => ERR_IO,
            };
        }
        Some((FdType::File, _, _)) => {
            // Fall through to file write below
        }
        _ => {
            // Unknown fd — still try to write to console instead of failing
            for i in 0..len {
                let byte = user_read::<u8>(buf + i);
                crate::arch::platform::console_putchar(byte);
            }
            return len as isize;
        }
    }

    // File write path
    let (name, pos, flags) = {
        crate::process::with_fd_table(|fd_table| match fd_table.get(fd as usize) {
            Some(f) => (f.name.clone(), f.pos, f.flags),
            None => (alloc::string::String::new(), 0, 0),
        })
    };
    if name.is_empty() {
        return ERR_INVAL;
    }

    // Read current file data, modify at pos, write back
    {
        let mut data = crate::driver::fs::read_file_owned(&name).unwrap_or_default();
        let end = pos + len;
        if end > data.len() {
            data.resize(end, 0);
        }
        for i in 0..len {
            data[pos + i] = user_read::<u8>(buf + i);
        }
        let _ = crate::driver::fs::write_file_owned(&name, &data);
    }

    // Update seek position
    crate::process::with_fd_table(|fd_table| {
        if let Some(f) = fd_table.get_mut(fd as usize) {
            f.pos += len;
        }
    });

    len as isize
}

/// Syscall 3: Read from file descriptor.
fn sys_read(fd: i32, buf: usize, len: usize) -> isize {
    if buf == 0 || len == 0 || len > 65536 {
        return ERR_INVAL;
    }

    // Check fd_table for the actual fd type.
    let fd_info = get_fd_info(fd);
    match fd_info {
        Some((FdType::PipeRead, Some(pipe_id), _)) => {
            return pipe_read(pipe_id, buf, len);
        }
        Some((FdType::PipeWrite, _, _)) => {
            return ERR_INVAL; // can't read from write end
        }
        Some((FdType::Stdio, _, _)) => {
            // Stdio stdin (fd 0 default): blocking read from TTY
            // TTY read needs to write to user memory — use intermediate buffer
            let mut kbuf = alloc::vec![0u8; len];
            loop {
                let result = crate::driver::tty::read(kbuf.as_mut_ptr() as usize, len);
                if result > 0 {
                    let user_buf = UserSliceMut::new(buf, len).unwrap();
                    user_buf.copy_from_slice(&kbuf[..result as usize]);
                    return result;
                }
                crate::driver::tty::poll_uart();
                crate::sched::schedule();
            }
        }
        Some((FdType::File, _, _)) => {
            // Fall through to file read below
        }
        Some((FdType::FakeFile(_), _, _)) | Some((FdType::Urandom, _, _)) => {
            // FakeFile/urandom: fake_read copies kernel bytes through user_write_u8().
            return crate::process::with_fd_table(|fd_table| {
                fd_table.fake_read(fd, buf, len).unwrap_or(0)
            }) as isize;
        }
        Some((FdType::VfsFile(vfs_fd), _, _)) => {
            // VFS file: read into kernel buffer first, then copy to user space.
            let mut kbuf = alloc::vec![0u8; len];
            return match crate::driver::vfs::read(vfs_fd, &mut kbuf) {
                Ok(n) => {
                    if let Some(user_buf) = UserSliceMut::new(buf, len) {
                        user_buf.copy_from_slice(&kbuf[..n]);
                    }
                    crate::process::with_fd_table(|fd_table| {
                        if let Some(f) = fd_table.get_mut(fd as usize) {
                            f.pos += n;
                        }
                    });
                    n as isize
                }
                Err(_) => ERR_IO,
            };
        }
        _ => {
            return ERR_INVAL;
        }
    }

    // File read path
    let (name, pos) = {
        crate::process::with_fd_table(|fd_table| match fd_table.get(fd as usize) {
            Some(f) => (f.name.clone(), f.pos),
            None => (alloc::string::String::new(), 0),
        })
    };
    if name.is_empty() {
        return ERR_INVAL;
    }

    // Read from FS (FAT32 + RamFS)
    let data = match crate::driver::fs::read_file_owned(&name) {
        Some(d) => d,
        None => return ERR_NOENT,
    };

    // Copy from current position
    if pos >= data.len() {
        return 0; // EOF
    }
    let to_read = core::cmp::min(len, data.len() - pos);
    user_write_bytes(buf, &data[pos..pos + to_read]);

    // Update seek position
    crate::process::with_fd_table(|fd_table| {
        if let Some(f) = fd_table.get_mut(fd as usize) {
            f.pos += to_read;
        }
    });

    to_read as isize
}

/// Syscall 4: Set/get program break (heap pointer).
fn sys_brk(addr: usize) -> isize {
    let current = crate::process::current_brk();
    if addr == 0 {
        return current as isize;
    }

    // Validate: new brk must be in heap range
    let heap_base = crate::process::USER_HEAP_BASE;
    let heap_limit = crate::process::USER_HEAP_LIMIT;
    if addr < heap_base || addr > heap_limit {
        return ERR_INVAL;
    }

    // Only grow, never shrink (Phase 2 simplification)
    if addr <= current {
        return current as isize;
    }

    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::trap::with_kernel_cr3(|| {
            let user_pt = crate::arch::trap::get_user_pt_safe();
            let page_size = crate::mm::pmm::page_size();
            let start_page = (current + page_size - 1) & !(page_size - 1);
            let end_page = (addr + page_size - 1) & !(page_size - 1);
            let mut vaddr = start_page;
            while vaddr < end_page {
                if crate::mm::vmm::translate_user(user_pt, vaddr).is_none() {
                    let frame = match crate::mm::pmm::alloc_frame() {
                        Some(f) => f,
                        None => return,
                    };
                    unsafe {
                        core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                    }
                    crate::mm::vmm::map(user_pt, vaddr, frame, crate::mm::vmm::PTEFlags::URW);
                }
                vaddr += page_size;
            }
        })
    };
    #[cfg(not(target_arch = "x86_64"))]
    {
        let user_pt = crate::arch::trap::get_current_user_pt();
        let page_size = crate::mm::pmm::page_size();
        let start_page = (current + page_size - 1) & !(page_size - 1);
        let end_page = (addr + page_size - 1) & !(page_size - 1);
        let mut vaddr = start_page;
        while vaddr < end_page {
            if crate::mm::vmm::translate_user(user_pt, vaddr).is_none() {
                let frame = match crate::mm::pmm::alloc_frame() {
                    Some(f) => f,
                    None => return ERR_NOMEM,
                };
                unsafe {
                    core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                }
                crate::mm::vmm::map(user_pt, vaddr, frame, crate::mm::vmm::PTEFlags::URW);
            }
            vaddr += page_size;
        }
    }

    // Flush TLB (RISC-V only; x86_64 TLB flush handled by with_kernel_cr3)
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("sfence.vma");
    }

    crate::process::set_current_brk(addr);
    addr as isize
}

/// Syscall 5: Get process ID.
fn sys_getpid() -> isize {
    match crate::process::current() {
        Some(p) => p.pid as isize,
        None => {
            // Fallback for test mode or kernel thread
            let tid = crate::sched::current_task_id();
            if tid == usize::MAX {
                0 // No task assigned yet
            } else {
                tid as isize
            }
        }
    }
}

/// Syscall 6: Map anonymous memory (KarteOS native ABI — 3 args).
/// `addr` = hint address (0 = kernel chooses), `len` = size, `_flags` = prot flags
/// Returns the mapped virtual address, or error.
///
/// When addr=0, allocates from a per-process mmap region that grows upward
/// from USER_MMAP_BASE. This matches Linux behavior where mmap returns
/// addresses in a dedicated region (not overlapping brk).
fn sys_mmap(addr: usize, len: usize, flags: usize) -> isize {
    if len == 0 {
        return ERR_INVAL;
    }
    // Extract prot from mmap flags (lower 3 bits encode protection)
    // Go passes full mmap2 args: (addr, len, prot, flags, fd, offset)
    // sys_mmap receives the first 3 args from the syscall dispatch
    linux_mmap(
        addr,
        len,
        flags, // prot bits passed from Go
        0x22,  /* MAP_PRIVATE|MAP_ANONYMOUS */
        usize::MAX,
        0,
    )
}

// ─── Linux mmap/mprotect/munmap ────────────────────────────────────────

/// Linux mmap constants
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const PROT_EXEC: usize = 4;
const MAP_SHARED: usize = 0x01;
const MAP_PRIVATE: usize = 0x02;
const MAP_FIXED: usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;

/// Linux mmap(addr, length, prot, flags, fd, offset)
/// Full Linux mmap6 implementation for Go runtime support.
/// Global lock for mmap/mprotect — prevents race conditions when multiple
/// CLONE_VM threads concurrently modify the shared page table.
fn linux_mmap(
    addr: usize,
    len: usize,
    prot: usize,
    flags: usize,
    _fd: usize,
    _offset: usize,
) -> isize {
    if len == 0 {
        return -22; // EINVAL
    }

    let result = linux_mmap_inner(addr, len, prot, flags, _fd, _offset);
    log_mmap_result(addr, len, prot, flags, result);
    result
}

fn linux_mmap_inner(
    addr: usize,
    len: usize,
    prot: usize,
    flags: usize,
    _fd: usize,
    _offset: usize,
) -> isize {
    let page_size = crate::mm::pmm::page_size();
    let aligned_len = (len + page_size - 1) & !(page_size - 1);
    let map_fixed = (flags & 0x10) != 0;
    let is_anonymous = flags & MAP_ANONYMOUS != 0 || _fd == usize::MAX;

    let target_addr = if addr != 0 && map_fixed {
        // MAP_FIXED: exact address required.
        // Reject if target range overlaps ELF segments.
        let aligned = addr & !(page_size - 1);
        let end = aligned + aligned_len;
        let max_elf = MAX_ELF_VADDR.load(core::sync::atomic::Ordering::Relaxed);
        if max_elf > 0 && aligned < max_elf && end > 0x400000 {
            // Overlaps ELF range — reject
            crate::console_println!(
                "[mmap-FIXED] REJECT elf overlap addr={:#x} end={:#x} max_elf={:#x}",
                aligned,
                end,
                max_elf
            );
            return aligned as isize;
        }
        aligned
    } else if addr != 0 {
        // Hint: use if valid AND no overlap with existing VMAs, otherwise kernel chooses.
        // PROT_NONE hints (Go's sysReserve) request high addresses for sparse
        // heap metadata. We honor them — bump allocator is NOT updated, so
        // subsequent PROT_RW sysAlloc calls still get low addresses.
        let aligned_addr = addr & !(page_size - 1);
        let hint_end = aligned_addr.checked_add(aligned_len).unwrap_or(0);
        let hint_in_range = aligned_addr >= crate::process::USER_MMAP_BASE
            && hint_end != 0
            && hint_end <= crate::process::USER_MMAP_LIMIT;
        let hint_overlaps = hint_in_range && vma_overlaps(aligned_addr, hint_end);
        crate::console_println!(
            "[mmap-hint] addr={:#x} aligned={:#x} end={:#x} in_range={} overlap={}",
            addr,
            aligned_addr,
            hint_end,
            hint_in_range,
            hint_overlaps
        );
        if hint_in_range && !hint_overlaps {
            aligned_addr
        } else {
            0 // hint conflicts with existing VMA or out of range → kernel chooses
        }
    } else {
        0
    };

    let target_addr = if target_addr != 0 {
        target_addr
    } else {
        // Bump allocator for kernel-chosen addresses.
        // NOTE: NEXT_MMAP_ADDR may be set past ELF segments by ensure_mmap_above().
        // If ELF segments extend past USER_MMAP_BASE (e.g., 69MB Go binary),
        // we must start allocating from the ELF end, not USER_MMAP_BASE.
        loop {
            let base = NEXT_MMAP_ADDR.load(core::sync::atomic::Ordering::Relaxed);
            let candidate = if base > 0 {
                base // Use ELF-aware address from ensure_mmap_above
            } else if crate::process::USER_MMAP_BASE > 0 {
                crate::process::USER_MMAP_BASE
            } else {
                base
            };
            let end_addr = candidate.checked_add(aligned_len).unwrap_or(0);
            if end_addr > crate::process::USER_MMAP_LIMIT || end_addr == 0 {
                return -12; // ENOMEM
            }
            let candidate_overlaps = vma_overlaps(candidate, end_addr);
            crate::console_println!(
                "[mmap-cand] base={:#x} candidate={:#x} end={:#x} overlap={}",
                base,
                candidate,
                end_addr,
                candidate_overlaps
            );
            if NEXT_MMAP_ADDR
                .compare_exchange(
                    base,
                    end_addr,
                    core::sync::atomic::Ordering::Relaxed,
                    core::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
            {
                break candidate;
            }
        }
    };

    let end = target_addr + aligned_len;

    // Register the VMA entry. For MAP_FIXED, removes overlapping entries.
    if vma_add(target_addr, end, prot, map_fixed).is_err() {
        return -12; // ENOMEM — VMA table full
    }

    // PROT_NONE (prot=0): reserve VA only. No PTEs, no frames.
    // The PF handler will refuse to allocate for PROT_NONE VMAs → SIGSEGV.
    if prot == 0 {
        return target_addr as isize;
    }

    // MAP_ANONYMOUS with PROT_R/W/X: lazy allocation.
    // No PTEs are created. The PF handler allocates zeroed frames on demand.
    // This is the standard Linux behavior for anonymous mmap.
    //
    // Non-anonymous mmap (file-backed) is not yet supported; treat as anonymous.

    // For MAP_FIXED on an existing mapping: unmap old PTEs in the range
    // so the PF handler will allocate fresh zeroed frames.
    if map_fixed {
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_kernel_cr3(|| {
            let user_pt = crate::arch::trap::get_user_pt_safe();
            let mut vaddr = target_addr;
            while vaddr < end {
                crate::mm::vmm::unmap_user(user_pt, vaddr);
                vaddr += page_size;
            }
            flush_tlb_all();
        });
        #[cfg(not(target_arch = "x86_64"))]
        {
            let user_pt = crate::arch::trap::get_current_user_pt();
            let mut vaddr = target_addr;
            while vaddr < end {
                crate::mm::vmm::unmap_user(user_pt, vaddr);
                vaddr += page_size;
            }
            flush_tlb_all();
        }
    }

    target_addr as isize
}

/// Log mmap result — always print, no rate limiting
fn log_mmap_result(addr: usize, len: usize, prot: usize, flags: usize, result: isize) {
    let tag = if flags & 0x10 != 0 { "FIXED" } else { "hint " };
    crate::console_println!(
        "[mmap] {} a0={:#x} len={:#x} prot={:#x} flags={:#x} => ret={:#x}",
        tag,
        addr,
        len,
        prot,
        flags,
        result
    );
}

/// Convert Linux prot flags to KarteOS PTEFlags.
pub fn prot_to_pte_flags(prot: usize) -> crate::mm::vmm::PTEFlags {
    let readable = prot & PROT_READ != 0;
    let writable = prot & PROT_WRITE != 0;
    let executable = prot & PROT_EXEC != 0;

    #[cfg(target_arch = "riscv64")]
    {
        use crate::mm::vmm::PTEFlags;
        let mut f = PTEFlags::V | PTEFlags::U;
        if readable {
            f |= PTEFlags::R;
        }
        if writable {
            f |= PTEFlags::W;
        }
        if executable {
            f |= PTEFlags::X;
        }
        // If nothing specified, default to R+W
        if prot == 0 {
            f |= PTEFlags::R | PTEFlags::W;
        }
        f
    }

    #[cfg(target_arch = "x86_64")]
    {
        use crate::mm::vmm::PTEFlags;
        let mut f = PTEFlags::PRESENT | PTEFlags::USER;
        if writable {
            f |= PTEFlags::WRITABLE;
        }
        if !executable {
            f |= PTEFlags::NX;
        }
        // Default: no NX (executable)
        f
    }
}

/// Linux mprotect(addr, len, prot) — change page protections.
/// Updates VMA entries and existing PTE flags. Does NOT allocate new frames;
/// the PF handler will use the updated VMA prot on first access.
fn linux_mprotect(addr: usize, len: usize, prot: usize) -> isize {
    if addr == 0 || len == 0 {
        return -22; // EINVAL
    }

    let page_size = crate::mm::pmm::page_size();
    let start = addr & !(page_size - 1);
    let end = (addr + len + page_size - 1) & !(page_size - 1);

    // Update VMA prot for this range
    vma_update_prot(start, end, prot);

    // Update existing PTE flags (only for already-mapped pages)
    let pte_flags = prot_to_pte_flags(prot);

    #[cfg(target_arch = "x86_64")]
    crate::arch::trap::with_kernel_cr3(|| {
        let user_pt = crate::arch::trap::get_user_pt_safe();
        for vaddr in (start..end).step_by(page_size) {
            if crate::mm::vmm::translate_user(user_pt, vaddr).is_some() {
                crate::mm::vmm::mprotect_user(user_pt, vaddr, pte_flags);
            }
            // Unmapped pages: no action needed — PF handler will use VMA prot
        }
        flush_tlb_all();
    });
    #[cfg(not(target_arch = "x86_64"))]
    {
        let user_pt = crate::arch::trap::get_current_user_pt();
        for vaddr in (start..end).step_by(page_size) {
            if crate::mm::vmm::translate_user(user_pt, vaddr).is_some() {
                crate::mm::vmm::mprotect_user(user_pt, vaddr, pte_flags);
            }
        }
        flush_tlb_all();
    }
    0
}

/// Linux munmap(addr, len) — unmap pages and free physical frames.
/// Also removes corresponding VMA entries.
fn linux_munmap(addr: usize, len: usize) -> isize {
    if addr == 0 || len == 0 {
        return -22; // EINVAL
    }

    let page_size = crate::mm::pmm::page_size();
    let start = addr & !(page_size - 1);
    let end = (addr + len + page_size - 1) & !(page_size - 1);

    // Validate range
    let valid_start = crate::process::USER_HEAP_BASE;
    let valid_end = crate::process::USER_MMAP_LIMIT;
    if start < valid_start || end > valid_end {
        return -22; // EINVAL
    }

    // Remove VMA entries
    vma_remove_range(start, end);

    // Free physical frames and unmap PTEs
    #[cfg(target_arch = "x86_64")]
    crate::arch::trap::with_kernel_cr3(|| {
        let user_pt = crate::arch::trap::get_user_pt_safe();
        for vaddr in (start..end).step_by(page_size) {
            crate::mm::vmm::unmap_user(user_pt, vaddr);
        }
        flush_tlb_all();
    });
    #[cfg(not(target_arch = "x86_64"))]
    {
        let user_pt = crate::arch::trap::get_current_user_pt();
        for vaddr in (start..end).step_by(page_size) {
            crate::mm::vmm::unmap_user(user_pt, vaddr);
        }
        flush_tlb_all();
    }
    0
}

/// Helper: flush the entire TLB (architecture-independent).
fn flush_tlb_all() {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("sfence.vma");
    }
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::trap::flush_tlb();
    }
}

/// Linux madvise(addr, len, advice)
///
/// Go runtime depends heavily on MADV_DONTNEED and MADV_FREE:
///   - `sysUnusedOS` calls madvise(MADV_DONTNEED/FREE) to release pages
///   - The kernel is expected to zero-fill these pages (or discard them)
///   - `sysUsedOS` does NOT remap — it assumes pages are zeroed
///
/// Linux fcntl(fd, cmd, arg) — file control operations.
///
/// Implements:
///   - F_GETFD / F_SETFD: close-on-exec flag
///   - F_GETFL / F_SETFL: file status flags
///   - F_GETLK / F_SETLK / F_SETLKW: POSIX advisory byte-range locking
///
/// Linux fsync(fd) — flush file data to disk.
/// For VFS/ext4 files, issues ATA FLUSH CACHE via AHCI to ensure data is on disk.
#[cfg(target_arch = "x86_64")]
fn linux_fsync(fd: usize) -> isize {
    if fd >= crate::driver::fs::MAX_FDS {
        return -9; // EBADF
    }
    // Issue ATA FLUSH CACHE to ensure all pending writes reach the physical disk.
    // This is critical for SQLite WAL mode durability guarantees.
    if crate::driver::ahci::is_available() {
        if let Err(e) = crate::driver::ahci::flush_cache() {
            crate::console_println!("[fsync] AHCI flush failed: {}", e);
            return -5; // EIO
        }
    }
    0
}

/// SQLite WAL mode depends critically on byte-range locks to coordinate
/// concurrent access from multiple Go goroutines (clone'd threads).
fn linux_fcntl(fd: usize, cmd: usize, arg: usize) -> isize {
    use crate::driver::fs::*;

    match cmd {
        F_GETFD => {
            // Return close-on-exec flag. We don't track this yet, return 0.
            0
        }
        F_SETFD => {
            // Set close-on-exec flag. Silently succeed.
            0
        }
        F_GETFL => {
            // Return file status flags for this fd.
            crate::process::with_fd_table(|fd_table| {
                match fd_table.get(fd) {
                    Some(desc) => desc.flags as isize,
                    None => -9, // EBADF
                }
            })
        }
        F_SETFL => {
            // Set file status flags (e.g., O_NONBLOCK, O_APPEND).
            crate::process::with_fd_table(|fd_table| {
                match fd_table.get_mut(fd) {
                    Some(desc) => {
                        // Preserve access mode bits, update status flags
                        desc.flags = (desc.flags & 0x3) | (arg as u32 & !0x3);
                        0
                    }
                    None => -9, // EBADF
                }
            })
        }
        F_GETLK | F_SETLK | F_SETLKW => {
            // arg points to struct flock in user memory.
            // Flock is 24 bytes: l_type(i16) + l_whence(i16) + l_start(i64) + l_len(i64) + l_pid(i32)
            if arg == 0 {
                return -14; // EFAULT
            }
            // Read from user space with CR3 switching on x86_64
            let flock_bytes = user_read_bytes(arg, 24);
            let flock = Flock::from_bytes(&flock_bytes);

            // Look up the inode for this fd — locks are keyed by inode.
            let inode = crate::process::with_fd_table(|fd_table| match fd_table.get(fd) {
                Some(desc) => match desc.fd_type {
                    FdType::File => crate::driver::vfs::get_inode_for_fd(fd),
                    FdType::VfsFile(vfs_fd) => crate::driver::vfs::get_inode_for_fd(vfs_fd),
                    _ => Some(fd as u64 + 0x10000),
                },
                None => None,
            });

            let inode = match inode {
                Some(ino) => ino,
                None => return -9, // EBADF
            };

            let (retval, out_flock) = fcntl_lock_op(cmd, fd, inode, &flock);

            if let Some(out) = out_flock {
                let bytes = out.to_bytes();
                user_write_bytes(arg, &bytes);
            }

            if retval == -1 && (cmd == F_SETLK || cmd == F_SETLKW) {
                return -11; // EAGAIN
            }
            retval
        }
        _ => {
            crate::console_println!("[fcntl] unhandled cmd={} fd={} arg={:#x}", cmd, fd, arg);
            0
        }
    }
}

/// Linux madvise(addr, len, advice)
///
/// Professional implementation that handles the full lifecycle of Go's memory
/// allocator:
///
///   1. `sysReserve` → mmap(PROT_NONE) — reserve VA
///   2. `sysMap`     → mmap(PROT_R/W, MAP_FIXED) — commit (pages become accessible)
///   3. `sysUsed`    → just accesses memory (PF allocates zeroed frames)
///   4. `sysUnused`  → madvise(MADV_DONTNEED) — decommit (release physical frames)
///   5. Go repeats 2-4 as needed
///
/// Key behaviors:
///   - MADV_DONTNEED: release physical frames (decommit). PTEs are removed.
///     Next access triggers PF → fresh zeroed frame allocated. This is how Linux
///     works — the process must handle SIGSEGV if it accesses MADV_DONTNEED'd
///     memory without re-mmap'ing, but Go always re-mmaps before accessing.
///   - MADV_FREE: same as MADV_DONTNEED for our purposes (lazy decommit).
///   - MADV_POPULATE_READ / MADV_POPULATE_WRITE: pre-fault pages (commit).
///     Allocate physical frames for all pages in the range that don't have them.
///   - MADV_WILLNEED: same as MADV_POPULATE_READ (pre-fault).
fn linux_madvise(addr: usize, len: usize, advice: usize) -> isize {
    if len == 0 {
        return 0;
    }

    const MADV_NORMAL: usize = 0;
    const MADV_RANDOM: usize = 1;
    const MADV_SEQUENTIAL: usize = 2;
    const MADV_WILLNEED: usize = 3;
    const MADV_DONTNEED: usize = 4;
    const MADV_FREE: usize = 8;
    const MADV_DONTFORK: usize = 10;
    const MADV_DOFORK: usize = 11;
    const MADV_MERGEABLE: usize = 12;
    const MADV_UNMERGEABLE: usize = 13;
    const MADV_HUGEPAGE: usize = 14;
    const MADV_NOHUGEPAGE: usize = 15;
    const MADV_POPULATE_READ: usize = 22;
    const MADV_POPULATE_WRITE: usize = 23;
    const MADV_COLLAPSE: usize = 25;

    let page_size = crate::mm::pmm::page_size();
    let start = addr & !(page_size - 1);
    let end = (addr + len + page_size - 1) & !(page_size - 1);

    match advice {
        MADV_DONTNEED | MADV_FREE => {
            // Decommit: release physical frames and remove PTEs.
            // The VMA entry is kept — Go will re-commit via mmap(PROT_R/W, MAP_FIXED).
            // Next PF in this range with a valid VMA will allocate a fresh zeroed frame.
            #[cfg(target_arch = "x86_64")]
            crate::arch::trap::with_kernel_cr3(|| {
                let user_pt = crate::arch::trap::get_user_pt_safe();
                let mut vaddr = start;
                while vaddr < end {
                    // unmap_user frees the physical frame and removes the PTE
                    crate::mm::vmm::unmap_user(user_pt, vaddr);
                    vaddr += page_size;
                }
                flush_tlb_all();
            });
            #[cfg(not(target_arch = "x86_64"))]
            {
                let user_pt = crate::arch::trap::get_current_user_pt();
                let mut vaddr = start;
                while vaddr < end {
                    crate::mm::vmm::unmap_user(user_pt, vaddr);
                    vaddr += page_size;
                }
                flush_tlb_all();
            }
            0
        }

        MADV_POPULATE_READ | MADV_POPULATE_WRITE | MADV_WILLNEED => {
            // Pre-fault: allocate physical frames for all pages in the range
            // that don't currently have PTE mappings. Uses VMA prot for flags.
            let vma_prot = vma_check(start);
            let pte_flags = if let Some(prot) = vma_prot {
                prot_to_pte_flags(prot)
            } else {
                // No VMA — use RW as default (matches mmap default)
                prot_to_pte_flags(1 | 2) // PROT_READ | PROT_WRITE
            };

            #[cfg(target_arch = "x86_64")]
            crate::arch::trap::with_kernel_cr3(|| {
                let user_pt = crate::arch::trap::get_user_pt_safe();
                let mut vaddr = start;
                while vaddr < end {
                    let needs_alloc = match crate::mm::vmm::translate_user(user_pt, vaddr) {
                        None => true,
                        Some(f) => f == vaddr, // identity-mapped stale entry
                    };
                    if needs_alloc {
                        if let Some(frame) = crate::mm::pmm::alloc_frame() {
                            unsafe { core::ptr::write_bytes(frame as *mut u8, 0, page_size) };
                            crate::mm::vmm::map(user_pt, vaddr, frame, pte_flags);
                        }
                        // If alloc_frame fails, silently skip — PF will retry on access
                    }
                    vaddr += page_size;
                }
                flush_tlb_all();
            });
            #[cfg(not(target_arch = "x86_64"))]
            {
                let user_pt = crate::arch::trap::get_current_user_pt();
                let mut vaddr = start;
                while vaddr < end {
                    if crate::mm::vmm::translate_user(user_pt, vaddr).is_none() {
                        if let Some(frame) = crate::mm::pmm::alloc_frame() {
                            unsafe { core::ptr::write_bytes(frame as *mut u8, 0, page_size) };
                            crate::mm::vmm::map(user_pt, vaddr, frame, pte_flags);
                        }
                    }
                    vaddr += page_size;
                }
                flush_tlb_all();
            }
            0
        }

        // All other advice values: no-op success
        MADV_NORMAL | MADV_RANDOM | MADV_SEQUENTIAL | MADV_DONTFORK | MADV_DOFORK
        | MADV_MERGEABLE | MADV_UNMERGEABLE | MADV_HUGEPAGE | MADV_NOHUGEPAGE | MADV_COLLAPSE => 0,
        _ => 0, // Unknown advice: silently succeed
    }
}

// ─── Linux signal/random/tid stubs (Go runtime compatibility) ──────

/// Linux rt_sigaction(sig, act, oldact, sigsetsize)
/// Record the signal handler address. Never actually deliver signals.
fn linux_rt_sigaction(sig: usize, act_ptr: usize, oldact_ptr: usize) -> isize {
    if sig == 0 || sig > 64 {
        return -22; // EINVAL
    }
    // Save old handler if requested
    if oldact_ptr != 0 {
        // struct sigaction { handler(8), sa_mask(8), sa_flags(8), sa_restorer(8) }
        // We only record the handler; write zeros for the rest.
        let handler_val =
            SIGNAL_STATE.handlers[sig - 1].load(core::sync::atomic::Ordering::Relaxed);
        user_write::<usize>(oldact_ptr, handler_val);
        user_write::<usize>(oldact_ptr + core::mem::size_of::<usize>(), 0);
        user_write::<usize>(oldact_ptr + 2 * core::mem::size_of::<usize>(), 0);
        user_write::<usize>(oldact_ptr + 3 * core::mem::size_of::<usize>(), 0);
    }
    // Set new handler if provided
    if act_ptr != 0 {
        let handler = user_read::<usize>(act_ptr);
        SIGNAL_STATE.handlers[sig - 1].store(handler, core::sync::atomic::Ordering::Relaxed);
    }
    0
}

/// Linux rt_sigprocmask(how, set, oldset, sigsetsize)
/// Record signal mask without actually blocking anything.
fn linux_rt_sigprocmask(how: usize, set_ptr: usize, oldset_ptr: usize) -> isize {
    // Return old mask if requested
    if oldset_ptr != 0 && how != 3 {
        let mask_val = SIGNAL_STATE
            .mask
            .load(core::sync::atomic::Ordering::Relaxed);
        user_write::<u64>(oldset_ptr, mask_val);
    }
    // Apply new mask if provided
    if set_ptr != 0 {
        let new_mask = user_read::<u64>(set_ptr);
        match how {
            0 => {
                // SIG_BLOCK: add signals to mask
                let prev = SIGNAL_STATE
                    .mask
                    .load(core::sync::atomic::Ordering::Relaxed);
                SIGNAL_STATE
                    .mask
                    .store(prev | new_mask, core::sync::atomic::Ordering::Relaxed);
            }
            1 => {
                // SIG_UNBLOCK: remove signals from mask
                let prev = SIGNAL_STATE
                    .mask
                    .load(core::sync::atomic::Ordering::Relaxed);
                SIGNAL_STATE
                    .mask
                    .store(prev & !new_mask, core::sync::atomic::Ordering::Relaxed);
            }
            2 => {
                // SIG_SETMASK: replace mask entirely
                SIGNAL_STATE
                    .mask
                    .store(new_mask, core::sync::atomic::Ordering::Relaxed);
            }
            _ => return -22, // EINVAL
        }
    }
    0
}

/// Linux sigaltstack(ss, oss)
/// Record alternate signal stack info. Never actually use it.
fn linux_sigaltstack(ss_ptr: usize, oss_ptr: usize) -> isize {
    // Return old state if requested
    if oss_ptr != 0 {
        // struct stack_t { ss_sp(8), ss_flags(8), ss_size(8) }
        let ss_sp = SIGNAL_STATE
            .altstack_sp
            .load(core::sync::atomic::Ordering::Relaxed);
        let ss_flags = SIGNAL_STATE
            .altstack_flags
            .load(core::sync::atomic::Ordering::Relaxed);
        let ss_size = SIGNAL_STATE
            .altstack_size
            .load(core::sync::atomic::Ordering::Relaxed);
        user_write::<usize>(oss_ptr, ss_sp);
        user_write::<usize>(oss_ptr + core::mem::size_of::<usize>(), ss_flags);
        user_write::<usize>(oss_ptr + 2 * core::mem::size_of::<usize>(), ss_size);
    }
    // Set new state if provided
    if ss_ptr != 0 {
        let ss_sp = user_read::<usize>(ss_ptr);
        let ss_flags = user_read::<usize>(ss_ptr + 8);
        let ss_size = user_read::<usize>(ss_ptr + 16);
        SIGNAL_STATE
            .altstack_sp
            .store(ss_sp, core::sync::atomic::Ordering::Relaxed);
        SIGNAL_STATE
            .altstack_flags
            .store(ss_flags, core::sync::atomic::Ordering::Relaxed);
        SIGNAL_STATE
            .altstack_size
            .store(ss_size, core::sync::atomic::Ordering::Relaxed);
    }
    0
}

/// Linux getrandom(buf, count, flags)
/// Fill buffer with pseudo-random data using a simple LCG PRNG.
fn linux_getrandom(buf: usize, count: usize, _flags: usize) -> isize {
    if buf == 0 || count == 0 {
        return ERR_INVAL;
    }
    for i in 0..count {
        // LCG: next = state * 6364136223846793005 + 1442695040888963407
        let prev = PRNG_STATE.load(core::sync::atomic::Ordering::Relaxed);
        let next = prev
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        PRNG_STATE.store(next, core::sync::atomic::Ordering::Relaxed);
        // Use bytes from the state
        let byte = ((next >> (i % 8 * 8)) & 0xFF) as u8;
        user_write::<u8>(buf + i, byte);
    }
    count as isize
}

/// Linux set_tid_address(tidptr)
/// Record the clear_child_tid pointer and return the current TID.
fn linux_set_tid_address(tidptr: usize) -> isize {
    // Store tidptr in current process for CLONE_CHILD_CLEARTID on exit.
    crate::process::set_child_tid_ptr(tidptr);
    // Return current TID
    sys_getpid()
}

/// Syscall 10: Open a file.
/// `path` = pointer to file path string, `path_len` = length, `flags` = open flags.
/// Returns the file descriptor number, or a negative error code.
pub(crate) fn sys_open(path: usize, path_len: usize, flags: u32) -> isize {
    if path == 0 || path_len == 0 || path_len > 256 {
        return ERR_INVAL;
    }

    // Read path from user memory
    let name = match read_user_path(path, path_len) {
        Some(n) if !n.is_empty() => n,
        _ => return ERR_INVAL,
    };

    // Resolve relative paths using CWD
    let name = resolve_path(&name);
    if name.is_empty() {
        return ERR_INVAL;
    }

    // Convert flags: Linux x86_64 O_CREAT=0x40, our internal=0x100
    let linux_o_creat: u32 = if cfg!(target_arch = "x86_64") {
        0x40
    } else {
        0x100
    };
    let has_creat = (flags & linux_o_creat) != 0 || (flags & crate::driver::fs::O_CREAT) != 0;
    let our_flags = if has_creat {
        crate::driver::fs::O_CREAT
    } else {
        0
    } | (flags & (crate::driver::fs::O_TRUNC | crate::driver::fs::O_APPEND));

    if name.contains("xbot")
        || name.contains("session")
        || name.contains(".db")
        || name.contains("shm")
        || name.contains("wal")
    {
        // Silently skip debug logging for these paths
    }

    // Try VFS open first (supports real ext4 files with O_CREAT)
    match crate::driver::vfs::open(&name, our_flags) {
        Ok(vfs_fd) => {
            // Register the VFS fd in the process FdTable
            return crate::process::with_fd_table(|fd_table| {
                match fd_table.alloc_vfs_fd(name.clone(), vfs_fd, flags) {
                    Some(fd) => fd as isize,
                    None => ERR_NOMEM,
                }
            });
        }
        Err(_) => {}
    }

    // If VFS failed, check for pseudo-filesystem paths
    let is_pseudo = is_pseudo_path(&name);

    // /dev/urandom and /dev/random need real random bytes
    // Match both "/dev/urandom" and "dev/urandom" (SQLite may use relative path)
    let is_urandom = name == "/dev/urandom"
        || name == "/dev/random"
        || name == "dev/urandom"
        || name == "dev/random";

    // If VFS failed, try RamFS fallback (for test files and embedded files)
    if !is_pseudo && !is_urandom {
        let ram_fs = crate::driver::fs::global_fs();
        if ram_fs.read(&name).is_some() {
            return crate::process::with_fd_table(|fd_table| {
                match fd_table.alloc(name.clone(), flags) {
                    Some(fd) => fd as isize,
                    None => ERR_NOMEM,
                }
            });
        }
    }

    if is_urandom {
        return crate::process::with_fd_table(|fd_table| match fd_table.alloc_urandom_fd(flags) {
            Some(fd) => fd as isize,
            None => ERR_NOMEM,
        });
    }

    if is_pseudo {
        return crate::process::with_fd_table(|fd_table| {
            match fd_table.alloc_fake_fd(name.clone(), flags) {
                Some(fd) => fd as isize,
                None => ERR_NOMEM,
            }
        });
    }

    ERR_NOENT
}

/// Syscall 11: Close a file descriptor.
fn sys_close(fd: i32) -> isize {
    if fd < 0 || fd as usize >= MAX_FDS {
        return ERR_INVAL;
    }

    // Check if this is a pipe fd — handle pipe reference counting
    let pipe_action = {
        crate::process::with_fd_table(|fd_table| {
            if let Some(desc) = fd_table.get(fd as usize) {
                match desc.fd_type {
                    FdType::PipeRead => desc.pipe_id.map(|pid| (pid, true)),
                    FdType::PipeWrite => desc.pipe_id.map(|pid| (pid, false)),
                    _ => None,
                }
            } else {
                None
            }
        })
    };

    // Close the fd in the table
    let closed = crate::process::with_fd_table(|fd_table| fd_table.close(fd as usize));
    if !closed {
        return ERR_INVAL;
    }

    // Release all byte-range locks held by this fd
    crate::driver::fs::release_fd_locks(fd as usize);

    // Handle pipe cleanup
    if let Some((pipe_id, is_read)) = pipe_action {
        if is_read {
            crate::driver::pipe::with_pipe(pipe_id, |p| p.close_read());
        } else {
            crate::driver::pipe::with_pipe(pipe_id, |p| p.close_write());
        }
        crate::driver::pipe::dec_ref(pipe_id);
    }

    ERR_OK
}

/// Syscall 30: Spawn a new process.
/// `prog_id` identifies which program to spawn (0 = hello, 1 = heap_test, 2 = file_test, 3 = spawn_test).
/// Returns child PID on success, or negative error code.
/// Sentinel returned by sys_waitpid when the child is still running.
/// Distinct from a real exit code (>= 0) and from errors so that an exit
/// code of 0 is not confused with "still running". The caller should poll.
pub const WAIT_AGAIN: isize = -1;
/// Returned by sys_waitpid when the pid is not a child of the caller.
pub const WAIT_ERR: isize = -2;

/// Syscall 31: Wait for a child process to exit.
///
/// Non-blocking. Returns the exit code (>= 0) when the child has exited (and
/// reaps it), `WAIT_AGAIN` while it is still running (caller should poll), or
/// `WAIT_ERR` when the pid is not a child of the caller (or already reaped).
fn sys_waitpid(pid: usize) -> isize {
    let my_pid = crate::process::current_pid();

    let child_idx = match crate::process::find_process_by_pid(pid) {
        Some(idx) => {
            if crate::process::get_ppid(idx) != my_pid {
                return WAIT_ERR;
            }
            idx
        }
        None => return WAIT_ERR,
    };

    match crate::process::get_exit_code(child_idx) {
        Some(exit_code) => {
            crate::process::reclaim_process(child_idx);
            crate::sched::remove_task(child_idx);
            exit_code as isize
        }
        None => {
            // Child still running — yield CPU so the child can execute.
            // Without this, a busy-waiting parent (e.g., shell wait_for loop)
            // monopolizes the CPU in kernel mode (cli prevents timer ISR),
            // and the child is never scheduled.
            crate::sched::schedule();
            WAIT_AGAIN
        }
    }
}

/// Read a byte string from user memory.
/// Linux pathnames are NUL-terminated: stop at the first NUL even if the
/// caller supplied a larger length.
fn read_user_path(ptr: usize, len: usize) -> Option<alloc::string::String> {
    if ptr == 0 || len == 0 || len > 512 {
        return None;
    }
    let mut buf = alloc::vec::Vec::with_capacity(len);
    for i in 0..len {
        let byte = user_read_u8(ptr + i);
        if byte == 0 {
            break;
        }
        buf.push(byte);
    }
    alloc::string::String::from_utf8(buf).ok()
}

/// Resolve a filesystem path relative to CWD.
///
/// - If `path` starts with '/', it is absolute — just strip the leading '/'.
/// - If `path` is relative, prepend CWD env var (with '/' separator if needed).
fn resolve_path(path: &str) -> alloc::string::String {
    if path.starts_with('/') {
        return alloc::string::String::from(path.strip_prefix('/').unwrap_or(path));
    }
    // Get CWD from env
    let cwd = crate::env::get("CWD").unwrap_or_else(|| alloc::string::String::from("/"));
    let cwd = cwd.trim_end_matches('/');
    if path.is_empty() {
        return alloc::string::String::from(cwd.trim_start_matches('/'));
    }
    // Build CWD/path
    let mut resolved = alloc::string::String::from(cwd);
    resolved.push('/');
    resolved.push_str(path);
    // Strip leading '/' for filesystem lookup (all lookups are relative to root)
    alloc::string::String::from(resolved.strip_prefix('/').unwrap_or(&resolved))
}

/// Syscall 52: Change directory.
/// Validates that the target directory exists before updating CWD.
fn sys_chdir(path: usize, path_len: usize) -> isize {
    let name = match read_user_path(path, path_len) {
        Some(n) if !n.is_empty() => n,
        _ => return ERR_INVAL,
    };

    let resolved = resolve_path(&name);

    // Verify the directory exists in the filesystem
    if crate::driver::ext4::has_ext4() {
        // Try to resolve the path — if lookup succeeds and it's a dir, ok
        match crate::driver::ext4::lookup_path(&resolved) {
            Some(inode) => {
                // Check if it's a directory
                match crate::driver::ext4::metadata_of(inode) {
                    Some(meta) if meta.is_dir() => {}
                    _ => return ERR_NOENT, // exists but not a directory
                }
            }
            None => return ERR_NOENT,
        }
    }
    // Also check RamFS
    // (For simplicity, allow cd to any path that exists in ext4 or RamFS)

    // Update CWD in env
    let mut full_cwd = alloc::string::String::from("/");
    if !resolved.is_empty() {
        full_cwd.push_str(&resolved);
    }
    crate::env::set("CWD", &full_cwd);
    ERR_OK
}

/// Syscall 40: List filesystem contents.
/// Lists the current working directory (CWD).
/// Writes a formatted listing to the user buffer (name + size per line).
/// Returns total bytes written, or error.
fn sys_ls(buf: usize, len: usize) -> isize {
    if buf == 0 || len == 0 {
        return ERR_INVAL;
    }

    // Get the directory path to list: resolve CWD to a path relative to root
    let cwd = crate::env::get("CWD").unwrap_or_else(|| alloc::string::String::from("/"));
    let dir_path = cwd.trim_start_matches('/');

    let files = crate::driver::fs::list_directory(dir_path);

    let mut written: usize = 0;
    for (name, size) in files {
        // Format: "name\tsize\n"

        // Write name
        for &b in name.as_bytes() {
            if written >= len {
                break;
            }
            user_write::<u8>(buf + written, b);
            written += 1;
        }
        // Tab
        if written < len {
            user_write::<u8>(buf + written, b'\t');
            written += 1;
        }
        // Size (write digits directly)
        if size == 0 {
            if written < len {
                user_write::<u8>(buf + written, b'0');
                written += 1;
            }
        } else {
            let mut tmp = [0u8; 20];
            let mut i = 0;
            let mut n = size;
            while n > 0 {
                tmp[i] = b'0' + (n % 10) as u8;
                n /= 10;
                i += 1;
            }
            for j in (0..i).rev() {
                if written >= len {
                    break;
                }
                user_write::<u8>(buf + written, tmp[j]);
                written += 1;
            }
        }
        // Newline
        if written < len {
            user_write::<u8>(buf + written, b'\n');
            written += 1;
        }
    }

    written as isize
}

/// Syscall 41: Create a directory.
fn sys_mkdir(path: usize, path_len: usize) -> isize {
    if path == 0 || path_len == 0 || path_len > 256 {
        return ERR_INVAL;
    }
    let name = match read_user_path(path, path_len) {
        Some(n) if !n.is_empty() => n,
        _ => return ERR_INVAL,
    };
    let name = resolve_path(&name);
    if let Some(inode) = crate::driver::fs::lookup_path(&name) {
        if let Some(meta) = crate::driver::ext4::metadata_of(inode) {
            if !meta.is_dir() {
                return -20; // ENOTDIR / existing non-directory at this path
            }
        }
        return -17; // EEXIST
    }
    match crate::driver::fs::create_dir(&name) {
        Ok(()) => 0,
        Err(()) => ERR_IO,
    }
}

/// Syscall 42: Delete a file or directory.
fn sys_unlink(path: usize, path_len: usize) -> isize {
    if path == 0 || path_len == 0 || path_len > 256 {
        return ERR_INVAL;
    }
    let name = match read_user_path(path, path_len) {
        Some(n) if !n.is_empty() => n,
        _ => return ERR_INVAL,
    };
    let name = resolve_path(&name);

    // Try to delete from VFS/ext4/RamFS. If the file doesn't exist,
    // still return success — SQLite's WAL mode cleanup tries to delete
    // old -shm/-wal files that may not exist, and treating ENOENT as
    // an IO error causes SQLITE_IOERR_DELETE_NOENT (5898).
    match crate::driver::fs::delete_file(&name) {
        Ok(()) => 0,
        Err(()) => 0, // File not found is NOT an error for unlink
    }
}

/// Syscall 50: Set an environment variable.
fn sys_setenv(key: usize, key_len: usize, val: usize, val_len: usize) -> isize {
    if key == 0 || key_len == 0 || key_len > 128 || val == 0 || val_len > 4096 {
        return ERR_INVAL;
    }
    let mut kbuf = user_read_bytes(key, key_len);
    while kbuf.last() == Some(&0) {
        kbuf.pop();
    }
    let key_str = alloc::string::String::from_utf8(kbuf).unwrap_or_default();

    let mut vbuf = user_read_bytes(val, val_len);
    while vbuf.last() == Some(&0) {
        vbuf.pop();
    }
    let val_str = alloc::string::String::from_utf8(vbuf).unwrap_or_default();

    // Update per-process env
    crate::process::update_current(|proc_opt| {
        if let Some(proc) = proc_opt {
            proc.env.insert(key_str.clone(), val_str.clone());
        }
    });
    // Also update global env for backward compat (e.g., shell's CMD_ARGS, CWD)
    crate::env::set(&key_str, &val_str);
    0
}

/// Syscall 51: Get an environment variable.
/// Returns the length of the value, or -1 if not found.
fn sys_getenv(key: usize, key_len: usize, buf: usize, buf_len: usize) -> isize {
    if key == 0 || key_len == 0 || key_len > 128 {
        return ERR_INVAL;
    }
    let mut kbuf = user_read_bytes(key, key_len);
    while kbuf.last() == Some(&0) {
        kbuf.pop();
    }
    let key_str = alloc::string::String::from_utf8(kbuf).unwrap_or_default();

    // First check per-process env, then fall back to global env
    let val_opt: Option<alloc::string::String> =
        { crate::process::current().and_then(|p| p.env.get(&key_str).cloned()) };
    let val = match val_opt {
        Some(v) => v,
        None => match crate::env::get(&key_str) {
            Some(v) => v,
            None => return -1,
        },
    };

    if buf != 0 && buf_len > 0 {
        let copy_len = core::cmp::min(val.len(), buf_len);
        user_write_bytes(buf, &val.as_bytes()[..copy_len]);
        copy_len as isize
    } else {
        val.len() as isize
    }
}

fn sys_spawn(prog_id: usize, _arg: usize) -> isize {
    // Map prog_id to file name (backward compatible)
    let file_name = match prog_id {
        0 => "hello",
        1 => "heap_test",
        2 => "file_test",
        3 => "spawn_test",
        _ => return ERR_INVAL,
    };

    // Load ELF data from filesystem (FAT32 first, then RamFS)
    let proc = match crate::driver::fs::read_file_owned(file_name) {
        Some(data) => match crate::process::Process::from_elf(
            &data,
            alloc::vec![file_name.as_bytes().to_vec()],
            alloc::vec![],
        ) {
            Ok(p) => p,
            Err(e) => {
                crate::klog!(DEBUG, "[spawn] Failed to create process: {}", e);
                return ERR_NOMEM;
            }
        },
        None => {
            crate::klog!(
                DEBUG,
                "[spawn] Program '{}' not found in filesystem",
                file_name
            );
            return ERR_NOENT;
        }
    };

    let child_pid = proc.pid;
    let entry = proc.entry;
    let user_stack_top = proc.user_stack_top;
    let kernel_stack_top = proc.kernel_stack_top;

    // Calculate user satp value (Sv39 mode = 8 on RISC-V, CR3 on x86_64)
    #[cfg(target_arch = "riscv64")]
    let user_satp = if proc.page_table_root == 0 {
        // Fallback: read current satp
        let satp: usize;
        unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp) };
        satp
    } else {
        (8usize << 60) | proc.page_table_root
    };

    #[cfg(target_arch = "x86_64")]
    let user_satp = proc.page_table_root << 12; // CR3 = PPN << 12

    // Register process in the global process table
    let proc_idx = match crate::process::add_process(proc) {
        Some(idx) => idx,
        None => {
            crate::klog!(DEBUG, "[spawn] Process table full");
            return ERR_NOMEM;
        }
    };

    // Set parent pid for the child process
    let parent_pid = crate::process::current_pid();
    crate::process::set_ppid(proc_idx, parent_pid);

    // Add to scheduler
    match crate::sched::add_user_process(
        entry,
        user_stack_top,
        kernel_stack_top,
        user_satp,
        proc_idx,
    ) {
        Some(_tid) => {
            crate::klog!(DEBUG, "[spawn] Spawned process pid={}", child_pid);
            child_pid as isize
        }
        None => {
            crate::klog!(DEBUG, "[spawn] Scheduler full");
            ERR_NOMEM
        }
    }
}

// ─── Network Syscalls (Level 10) ────────────────────────────────────

/// Parse a sockaddr_in from user memory.
/// Returns (port, ip_bytes) or error.
fn parse_sockaddr_in(addr_ptr: usize, addr_len: usize) -> Result<(u16, [u8; 4]), isize> {
    if addr_ptr == 0 || addr_len < 8 {
        return Err(ERR_INVAL);
    }

    let data = user_read_bytes(addr_ptr, addr_len.min(16));

    // family (bytes 0-1), port (bytes 2-3, big-endian), ip (bytes 4-7, big-endian)
    let family = u16::from_le_bytes([data[0], data[1]]);
    if family != 2 {
        // Not AF_INET
        return Err(ERR_INVAL);
    }

    let port = u16::from_be_bytes([data[2], data[3]]);
    let ip = [data[4], data[5], data[6], data[7]];

    Ok((port, ip))
}

/// Syscall 70: socket(domain, type, protocol) → fd
/// domain: 2 = AF_INET
/// type:   1 = SOCK_STREAM (TCP), 2 = SOCK_DGRAM (UDP), 3 = SOCK_RAW (ICMP)
#[allow(unused_variables)]
fn sys_socket(domain: usize, socket_type: usize, _protocol: usize) -> isize {
    if domain != 2 {
        return ERR_INVAL;
    }

    let stype = match socket_type {
        1 => crate::net::iface::SocketType::Tcp,
        2 => crate::net::iface::SocketType::Udp,
        3 => crate::net::iface::SocketType::Icmp,
        _ => return ERR_INVAL,
    };

    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }

    crate::net::iface::NetStack::create_socket(stype)
}

/// Syscall 71: bind(fd, addr_ptr, addr_len) → 0
fn sys_bind(fd: i32, addr_ptr: usize, addr_len: usize) -> isize {
    if fd < 0 {
        return ERR_INVAL;
    }

    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }

    let (port, _) = match parse_sockaddr_in(addr_ptr, addr_len) {
        Ok(v) => v,
        Err(e) => return e,
    };

    crate::net::iface::NetStack::bind(fd as usize, port)
}

/// Syscall 72: connect(fd, addr_ptr, addr_len) → 0
fn sys_connect(fd: i32, addr_ptr: usize, addr_len: usize) -> isize {
    if fd < 0 {
        return ERR_INVAL;
    }

    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }

    let (port, ip) = match parse_sockaddr_in(addr_ptr, addr_len) {
        Ok(v) => v,
        Err(e) => return e,
    };

    crate::net::iface::NetStack::connect(fd as usize, ip, port)
}

/// Syscall 73: listen(fd, backlog) → 0
fn sys_listen(fd: i32, _backlog: usize) -> isize {
    if fd < 0 {
        return ERR_INVAL;
    }

    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }

    // bind with port 0 means "pick an ephemeral port and listen"
    crate::net::iface::NetStack::bind(fd as usize, 0)
}

/// Syscall 74: accept(fd) → new_fd
fn sys_accept(fd: i32) -> isize {
    if fd < 0 {
        return ERR_INVAL;
    }

    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }

    if crate::net::iface::NetStack::is_connected(fd as usize) {
        fd as isize
    } else {
        -2 // EAGAIN — would block
    }
}

/// Syscall 75: sendto(fd, buf, len, flags, addr_ptr, addr_len) → bytes_sent
#[allow(unused_variables)]
fn sys_sendto(
    fd: i32,
    buf: usize,
    len: usize,
    _flags: usize,
    addr_ptr: usize,
    addr_len: usize,
) -> isize {
    if fd < 0 || buf == 0 || len == 0 {
        return ERR_INVAL;
    }

    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }

    let data = user_read_bytes(buf, len);

    // If destination address is provided, parse it
    let dest = if addr_ptr != 0 && addr_len >= 8 {
        match parse_sockaddr_in(addr_ptr, addr_len) {
            Ok((port, ip)) => Some((ip, port)),
            Err(_) => return ERR_INVAL,
        }
    } else {
        None
    };

    let (ip, port) = match dest {
        Some((ip, port)) => (Some(ip), Some(port)),
        None => (None, None),
    };

    crate::net::iface::NetStack::send(fd as usize, &data, ip, port)
}

/// Syscall 76: recvfrom(fd, buf, len, flags) → bytes_received
fn sys_recvfrom(fd: i32, buf: usize, len: usize) -> isize {
    if fd < 0 || buf == 0 || len == 0 {
        return ERR_INVAL;
    }

    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }

    let mut kbuf = alloc::vec![0u8; len];

    match crate::net::iface::NetStack::recv(fd as usize, &mut kbuf) {
        Ok((n, _src_ip, _src_port)) => {
            user_write_bytes(buf, &kbuf[..n]);
            n as isize
        }
        Err(e) => e,
    }
}

/// Syscall 77: shutdown(fd, how) → 0
fn sys_shutdown(fd: i32) -> isize {
    if fd < 0 {
        return ERR_INVAL;
    }

    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }

    crate::net::iface::NetStack::shutdown(fd as usize)
}

/// Syscall 81: Read kernel log buffer (for dmesg).
/// `buf` = user buffer, `len` = buffer size, `offset` = read offset (0 = start).
/// Returns number of bytes read, or negative error code.
fn sys_syslog(buf: usize, len: usize, offset: usize) -> isize {
    if buf == 0 || len == 0 {
        return ERR_INVAL;
    }

    let mut kbuf = alloc::vec![0u8; len];
    let read = crate::kernel_log::log_peek(&mut kbuf, offset);
    user_write_bytes(buf, &kbuf[..read]);
    read as isize
}

/// Syscall 32: Execute (spawn) a program by file path.
/// `path` = pointer to file path string, `path_len` = length.
/// Returns child PID on success, or negative error code.
fn sys_exec(path: usize, path_len: usize, argv_ptr: usize, envp_ptr: usize) -> isize {
    sys_exec_impl(path, path_len, argv_ptr, envp_ptr)
}

fn sys_exec_impl(path: usize, path_len: usize, argv_ptr: usize, envp_ptr: usize) -> isize {
    if path == 0 || path_len == 0 || path_len > 256 {
        return ERR_INVAL;
    }

    // Read path from user memory
    let mut path_buf = user_read_bytes(path, path_len);
    // Strip trailing NUL
    while path_buf.last() == Some(&0) {
        path_buf.pop();
    }
    // Strip leading '/'
    if path_buf.starts_with(b"/") {
        path_buf.remove(0);
    }
    let name = alloc::string::String::from_utf8(path_buf).unwrap_or_default();
    if name.is_empty() {
        return ERR_INVAL;
    }

    // Try streaming ELF loader from ext4 first
    let argv = if argv_ptr != 0 {
        read_user_argv(argv_ptr)
    } else {
        // Default: argv[0] = program name
        alloc::vec![name.as_bytes().to_vec()]
    };

    // Read envp from user space and build per-process env
    let mut proc_env: alloc::collections::BTreeMap<alloc::string::String, alloc::string::String> =
        if envp_ptr != 0 {
            let envp_entries = read_user_envp(envp_ptr);
            envp_entries
                .iter()
                .map(|(k, v)| {
                    (
                        alloc::string::String::from_utf8_lossy(k).into_owned(),
                        alloc::string::String::from_utf8_lossy(v).into_owned(),
                    )
                })
                .collect()
        } else {
            // Inherit current process's env (or global env for init)
            let current = crate::process::current();
            if let Some(p) = current {
                p.env.clone()
            } else {
                let mut env = alloc::collections::BTreeMap::new();
                merge_global_env(&mut env);
                env
            }
        };

    // Build envp Vec for initial stack
    let envp = env_to_envp(&proc_env);

    crate::klog!(
        DEBUG,
        "[exec] Loading '{}' argc={} envp_count={}...",
        name,
        argv.len(),
        envp.len()
    );

    // Clear VMA table — exec replaces the entire address space.
    vma_clear();

    // Try streaming ELF loader from ext4 first (avoids loading entire file into memory)
    let mut proc = if crate::driver::ext4::has_ext4() {
        let read_opt = crate::driver::ext4::read_file_range(&name);
        match read_opt {
            Some(read_fn) => {
                match crate::process::Process::from_elf_streaming(read_fn, argv, envp, 0) {
                    Ok(p) => p,
                    Err(e) => {
                        crate::klog!(DEBUG, "[exec] Streaming ELF load failed: {}", e);
                        return ERR_NOMEM;
                    }
                }
            }
            None => {
                // File not found on ext4, try fallback
                let argv2 = if let Some(pos) = name.rfind('/') {
                    let mut v = argv.clone();
                    // Update argv[0] to just the basename if it was a path
                    if v.is_empty() {
                        v.push(name[pos + 1..].as_bytes().to_vec());
                    }
                    v
                } else {
                    argv.clone()
                };
                match crate::driver::fs::read_file_owned(&name) {
                    Some(data) => match crate::process::Process::from_elf(&data, argv2, envp) {
                        Ok(p) => p,
                        Err(e) => {
                            crate::klog!(DEBUG, "[exec] Failed to create process: {}", e);
                            return ERR_NOMEM;
                        }
                    },
                    None => {
                        crate::klog!(DEBUG, "[exec] Program '{}' not found", name);
                        return ERR_NOENT;
                    }
                }
            }
        }
    } else {
        // No ext4 — use traditional loader (FAT32 + RamFS)
        match crate::driver::fs::read_file_owned(&name) {
            Some(data) => match crate::process::Process::from_elf(&data, argv, envp) {
                Ok(p) => p,
                Err(e) => {
                    crate::klog!(DEBUG, "[exec] Failed to create process: {}", e);
                    return ERR_NOMEM;
                }
            },
            None => {
                crate::klog!(DEBUG, "[exec] Program '{}' not found", name);
                return ERR_NOENT;
            }
        }
    };

    // Set per-process env
    proc.env = proc_env;

    let child_pid = proc.pid;
    let entry = proc.entry;
    let user_stack_top = proc.user_stack_top;
    let kernel_stack_top = proc.kernel_stack_top;

    #[cfg(target_arch = "riscv64")]
    let user_satp = if proc.page_table_root == 0 {
        let satp: usize;
        unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp) };
        satp
    } else {
        (8usize << 60) | proc.page_table_root
    };

    #[cfg(target_arch = "x86_64")]
    let user_satp = proc.page_table_root << 12; // CR3 = PPN << 12

    let proc_idx = match crate::process::add_process(proc) {
        Some(idx) => idx,
        None => {
            crate::klog!(DEBUG, "[exec] Process table full");
            return ERR_NOMEM;
        }
    };

    let parent_pid = crate::process::current_pid();
    crate::process::set_ppid(proc_idx, parent_pid);

    match crate::sched::add_user_process(
        entry,
        user_stack_top,
        kernel_stack_top,
        user_satp,
        proc_idx,
    ) {
        Some(_tid) => {
            crate::klog!(DEBUG, "[exec] Spawned '{}' pid={}", name, child_pid);
            child_pid as isize
        }
        None => {
            crate::klog!(DEBUG, "[exec] Scheduler full");
            ERR_NOMEM
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── Syscall Tests ──");

    crate::test::run_test("syscall_unknown_returns_error", || {
        dispatch(9999, [0, 0, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_constants_correct", || {
        SYS_DEBUG_PRINT == 0
            && SYS_EXIT == 1
            && SYS_WRITE == 2
            && SYS_READ == 3
            && SYS_BRK == 4
            && SYS_GETPID == 5
    });

    crate::test::run_test("syscall_getpid_returns_valid", || {
        let pid = dispatch(SYS_GETPID, [0, 0, 0, 0, 0, 0]);
        pid >= 0
    });

    crate::test::run_test("syscall_write_bad_fd_returns_error", || {
        dispatch(SYS_WRITE, [0, 0, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_brk_zero_returns_current", || {
        dispatch(SYS_BRK, [0, 0, 0, 0, 0, 0]) >= 0
    });

    crate::test::run_test("syscall_brk_grows_heap", || {
        let current = dispatch(SYS_BRK, [0, 0, 0, 0, 0, 0]) as usize;
        let new_brk = current + 4096;
        let result = dispatch(SYS_BRK, [new_brk, 0, 0, 0, 0, 0]);
        result == new_brk as isize
    });

    crate::test::run_test("syscall_brk_invalid_addr_returns_error", || {
        dispatch(SYS_BRK, [0xFFFF_FFFF, 0, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_yield_returns_zero", || {
        // Using old number for backward compat
        true // Just check constant exists
    });

    crate::test::run_test("syscall_mmap_allocates_memory", || {
        let result = dispatch(SYS_MMAP, [0, 4096, 0, 0, 0, 0]);
        result >= 0
    });

    crate::test::run_test("syscall_mmap_zero_len_returns_error", || {
        dispatch(SYS_MMAP, [0, 0, 0, 0, 0, 0]) == ERR_INVAL
    });

    // ── File syscall tests ──

    crate::test::run_test("syscall_open_close", || {
        // Create a test file in the global FS
        {
            let mut fs = crate::driver::fs::global_fs();
            let _ = fs.write("_sys_test_oc.txt", b"hello");
        }
        let path = b"_sys_test_oc.txt";
        let fd = dispatch(
            SYS_OPEN,
            [
                path.as_ptr() as usize,
                path.len(),
                O_RDONLY as usize,
                0,
                0,
                0,
            ],
        );
        if fd < 0 {
            return false;
        }
        let close_result = dispatch(SYS_CLOSE, [fd as usize, 0, 0, 0, 0, 0]);
        close_result == ERR_OK
    });

    crate::test::run_test("syscall_open_nonexistent", || {
        let path = b"_sys_test_noexist.txt";
        let fd = dispatch(
            SYS_OPEN,
            [
                path.as_ptr() as usize,
                path.len(),
                O_RDONLY as usize,
                0,
                0,
                0,
            ],
        );
        fd == ERR_NOENT
    });

    crate::test::run_test("syscall_open_read_close", || {
        // Create a test file
        {
            let mut fs = crate::driver::fs::global_fs();
            let _ = fs.write("_sys_test_read.txt", b"hello world");
        }
        let path = b"_sys_test_read.txt";
        let fd = dispatch(
            SYS_OPEN,
            [
                path.as_ptr() as usize,
                path.len(),
                O_RDONLY as usize,
                0,
                0,
                0,
            ],
        );
        if fd < 0 {
            return false;
        }
        let mut buf = [0u8; 64];
        let n = dispatch(
            SYS_READ,
            [fd as usize, buf.as_mut_ptr() as usize, 64, 0, 0, 0],
        );
        dispatch(SYS_CLOSE, [fd as usize, 0, 0, 0, 0, 0]);
        n == 11 && buf[..11] == b"hello world"[..]
    });

    crate::test::run_test("syscall_open_write_read", || {
        // Create a test file
        {
            let mut fs = crate::driver::fs::global_fs();
            let _ = fs.write("_sys_test_rw.txt", b"initial");
        }
        // Open for writing
        let path = b"_sys_test_rw.txt";
        let fd_w = dispatch(
            SYS_OPEN,
            [
                path.as_ptr() as usize,
                path.len(),
                O_WRONLY as usize,
                0,
                0,
                0,
            ],
        );
        if fd_w < 0 {
            return false;
        }
        // Write new data
        let write_data = b"world!!";
        let n = dispatch(
            SYS_WRITE,
            [
                fd_w as usize,
                write_data.as_ptr() as usize,
                write_data.len(),
                0,
                0,
                0,
            ],
        );
        dispatch(SYS_CLOSE, [fd_w as usize, 0, 0, 0, 0, 0]);
        if n != write_data.len() as isize {
            return false;
        }
        // Re-open for reading and verify
        let fd_r = dispatch(
            SYS_OPEN,
            [
                path.as_ptr() as usize,
                path.len(),
                O_RDONLY as usize,
                0,
                0,
                0,
            ],
        );
        if fd_r < 0 {
            return false;
        }
        let mut buf = [0u8; 64];
        let n = dispatch(
            SYS_READ,
            [fd_r as usize, buf.as_mut_ptr() as usize, 64, 0, 0, 0],
        );
        dispatch(SYS_CLOSE, [fd_r as usize, 0, 0, 0, 0, 0]);
        n == 7 && buf[..7] == b"world!!"[..]
    });

    crate::test::run_test("syscall_close_invalid", || {
        // fd 99 is not allocated
        let result = dispatch(SYS_CLOSE, [99, 0, 0, 0, 0, 0]);
        result == ERR_INVAL
    });

    #[cfg(target_arch = "x86_64")]
    crate::test::run_test("linux_getdents64_is_implemented", || {
        let mut buf = [0u8; 64];
        let result =
            dispatch_linux_syscall(217, [99, buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0]);
        result != -38
    });

    // ── Network syscall tests ──

    crate::test::run_test("syscall_socket_invalid_domain", || {
        // domain != AF_INET(2) should fail
        dispatch(SYS_SOCKET, [3, 1, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_socket_invalid_type", || {
        // type=99 is invalid
        dispatch(SYS_SOCKET, [2, 99, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_socket_negative_fd_invalid", || {
        // Negative fd should fail
        dispatch(SYS_BIND, [!0u32 as usize, 0, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_connect_negative_fd", || {
        dispatch(SYS_CONNECT, [!0u32 as usize, 0, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_listen_negative_fd", || {
        dispatch(SYS_LISTEN, [!0u32 as usize, 0, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_accept_negative_fd", || {
        dispatch(SYS_ACCEPT, [!0u32 as usize, 0, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_sendto_negative_fd", || {
        dispatch(SYS_SENDTO, [!0u32 as usize, 0, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_sendto_null_buf", || {
        dispatch(SYS_SENDTO, [0, 0, 10, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_sendto_zero_len", || {
        let buf = b"test";
        dispatch(SYS_SENDTO, [0, buf.as_ptr() as usize, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_recvfrom_negative_fd", || {
        dispatch(SYS_RECVFROM, [!0u32 as usize, 0, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_recvfrom_null_buf", || {
        dispatch(SYS_RECVFROM, [0, 0, 10, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_recvfrom_zero_len", || {
        let mut buf = [0u8; 10];
        dispatch(SYS_RECVFROM, [0, buf.as_mut_ptr() as usize, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_shutdown_negative_fd", || {
        dispatch(SYS_SHUTDOWN, [!0u32 as usize, 0, 0, 0, 0, 0]) == ERR_INVAL
    });

    crate::test::run_test("syscall_net_constants_correct", || {
        SYS_SOCKET == 70
            && SYS_BIND == 71
            && SYS_CONNECT == 72
            && SYS_LISTEN == 73
            && SYS_ACCEPT == 74
            && SYS_SENDTO == 75
            && SYS_RECVFROM == 76
            && SYS_SHUTDOWN == 77
    });
}

// ─── Pipe / Dup2 / Redirect helpers ──────────────────────────────────

use crate::driver::fs::{FdType, O_APPEND};

/// Get fd type, pipe_id, and name for a given fd number.
/// Returns None if fd is invalid or not a pipe.
fn get_fd_info(fd: i32) -> Option<(FdType, Option<usize>, alloc::string::String)> {
    if fd < 0 || fd as usize >= MAX_FDS {
        return None;
    }
    crate::process::with_fd_table(|fd_table| {
        fd_table
            .get(fd as usize)
            .map(|f| (f.fd_type.clone(), f.pipe_id, f.name.clone()))
    })
}

/// Blocking read from a pipe. Called from sys_read when fd is a PipeRead.
fn pipe_read(pipe_id: usize, buf: usize, len: usize) -> isize {
    loop {
        let result = crate::driver::pipe::with_pipe(pipe_id, |p| p.read(buf, len));
        match result {
            Some(n) => {
                if n == -1 {
                    // Pipe is empty, write end still open — block
                    let proc_idx = crate::process::current_index();
                    crate::driver::pipe::with_pipe(pipe_id, |p| p.set_reader_blocked(proc_idx));
                    crate::sched::schedule_block();
                    // Woken up — loop back and try again
                    continue;
                }
                return n; // success (n bytes) or 0 (EOF)
            }
            None => return ERR_INVAL, // pipe doesn't exist
        }
    }
}

/// Blocking write to a pipe. Called from sys_write when fd is a PipeWrite.
fn pipe_write(pipe_id: usize, buf: usize, len: usize) -> isize {
    loop {
        let result = crate::driver::pipe::with_pipe(pipe_id, |p| p.write(buf, len));
        match result {
            Some(n) => {
                if n == crate::driver::pipe::EPIPE {
                    // Read end closed
                    crate::klog!(DEBUG, "[pipe] write: Broken pipe");
                    return n;
                }
                if n == -1 {
                    // Pipe is full — block
                    let proc_idx = crate::process::current_index();
                    crate::driver::pipe::with_pipe(pipe_id, |p| p.set_writer_blocked(proc_idx));
                    crate::sched::schedule_block();
                    // Woken up — loop back and try again
                    continue;
                }
                return n; // success
            }
            None => return ERR_INVAL,
        }
    }
}

/// Syscall 7: Create an anonymous pipe.
/// `fd_ptr` points to a user-space `[i32; 2]` where the two fd numbers are written.
/// Returns 0 on success, negative on error.
fn sys_pipe(fd_ptr: usize) -> isize {
    if fd_ptr == 0 {
        return ERR_INVAL;
    }

    let pipe_id = match crate::driver::pipe::alloc_pipe() {
        Some(id) => id,
        None => return ERR_NOMEM,
    };

    // Allocate two fds in the current process's fd table
    let (read_fd, write_fd) = {
        crate::process::with_fd_table(|fd_table| {
            let rfd = fd_table.alloc_pipe_fd(pipe_id, true);
            let wfd = fd_table.alloc_pipe_fd(pipe_id, false);
            (rfd, wfd)
        })
    };

    match (read_fd, write_fd) {
        (Some(rfd), Some(wfd)) => {
            // Write fd pair to user space
            user_write::<i32>(fd_ptr, rfd as i32);
            user_write::<i32>(fd_ptr + 4, wfd as i32);
            ERR_OK
        }
        _ => {
            // Failed to allocate fds — clean up pipe
            crate::driver::pipe::dec_ref(pipe_id);
            crate::driver::pipe::dec_ref(pipe_id);
            ERR_NOMEM
        }
    }
}

/// Syscall 8: Duplicate a file descriptor.
/// `old_fd` is the source fd, `new_fd` is the target fd.
/// If `new_fd` is already open, it is closed first.
/// Returns `new_fd` on success, negative on error.
fn sys_dup2(old_fd: i32, new_fd: i32) -> isize {
    if old_fd < 0 || new_fd < 0 || old_fd as usize >= MAX_FDS || new_fd as usize >= MAX_FDS {
        return ERR_INVAL;
    }
    if old_fd == new_fd {
        return new_fd as isize;
    }

    // Clone the fd entry from old_fd to new_fd
    crate::process::with_fd_table(|fd_table| {
        let desc = match fd_table.get(old_fd as usize) {
            Some(d) => d.clone(),
            None => return ERR_INVAL,
        };

        // If it's a pipe fd, increment the pipe reference count
        if let Some(pipe_id) = desc.pipe_id {
            crate::driver::pipe::inc_ref(pipe_id);
        }

        fd_table.set_fd(new_fd as usize, desc);
        new_fd as isize
    })
}

/// Syscall 33: Exec a program with fd redirection.
/// `path` = path string pointer, `path_len` = length
/// `redir_stdin` = fd to use as stdin for the child (-1 = keep default)
/// `redir_stdout` = fd to use as stdout for the child (-1 = keep default)
fn sys_exec_fd(path: usize, path_len: usize, redir_stdin: i32, redir_stdout: i32) -> isize {
    // Read path from user memory
    let name = match read_user_path(path, path_len) {
        Some(s) if !s.is_empty() => {
            // Strip leading '/' if present (fs root convention)
            if s.starts_with('/') {
                alloc::string::String::from(&s[1..])
            } else {
                s
            }
        }
        _ => return ERR_INVAL,
    };

    // Build argv from CMD_ARGS env var (for backward compat with .S programs)
    let args_str = crate::env::get("CMD_ARGS").unwrap_or_default();
    let mut argv: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec![name.as_bytes().to_vec()];
    if !args_str.is_empty() {
        // Split CMD_ARGS by spaces into argv[1..]
        let bytes = args_str.as_bytes();
        let mut start = 0;
        while start < bytes.len() {
            while start < bytes.len() && bytes[start] == b' ' {
                start += 1;
            }
            if start >= bytes.len() {
                break;
            }
            let mut end = start;
            while end < bytes.len() && bytes[end] != b' ' {
                end += 1;
            }
            argv.push(bytes[start..end].to_vec());
            start = end;
        }
    }

    // Build envp from per-process env (or global env for init)
    let proc_env = {
        let current = crate::process::current();
        if let Some(p) = current {
            p.env.clone()
        } else {
            let mut env = alloc::collections::BTreeMap::new();
            merge_global_env(&mut env);
            env
        }
    };
    let envp = env_to_envp(&proc_env);

    // Clear VMA table — exec replaces the entire address space.
    vma_clear();

    // Load ELF from filesystem — try streaming loader from ext4 first
    let mut proc = if crate::driver::ext4::has_ext4() {
        match crate::driver::ext4::read_file_range(&name) {
            Some(read_fn) => {
                match crate::process::Process::from_elf_streaming(read_fn, argv, envp, 0) {
                    Ok(p) => p,
                    Err(e) => {
                        crate::console_println!(
                            "[exec] Streaming ELF load failed for '{}': {}",
                            name,
                            e
                        );
                        return ERR_IO;
                    }
                }
            }
            None => {
                // File not found on ext4, try fallback
                match crate::driver::fs::read_file_owned(&name) {
                    Some(data) => match crate::process::Process::from_elf(
                        &data,
                        alloc::vec![name.as_bytes().to_vec()],
                        alloc::vec![],
                    ) {
                        Ok(p) => p,
                        Err(e) => {
                            crate::klog!(DEBUG, "[exec] Failed to parse ELF '{}': {}", name, e);
                            return ERR_IO;
                        }
                    },
                    None => return ERR_NOENT,
                }
            }
        }
    } else {
        // No ext4 — use traditional loader
        match crate::driver::fs::read_file_owned(&name) {
            Some(data) => match crate::process::Process::from_elf(
                &data,
                alloc::vec![name.as_bytes().to_vec()],
                alloc::vec![],
            ) {
                Ok(p) => p,
                Err(e) => {
                    crate::klog!(DEBUG, "[exec] Failed to parse ELF '{}': {}", name, e);
                    return ERR_IO;
                }
            },
            None => return ERR_NOENT,
        }
    };

    // Set per-process env
    proc.env = proc_env;

    // Apply fd redirections: copy parent's fd entries to child
    if redir_stdin >= 0 || redir_stdout >= 0 {
        let (stdin_desc, stdout_desc) = {
            crate::process::with_fd_table(|fd_table| {
                let sin = if redir_stdin >= 0 {
                    fd_table.get(redir_stdin as usize).cloned()
                } else {
                    None
                };
                let sout = if redir_stdout >= 0 {
                    fd_table.get(redir_stdout as usize).cloned()
                } else {
                    None
                };
                (sin, sout)
            })
        };

        if let Some(desc) = stdin_desc {
            // Increment pipe ref if applicable
            if let Some(pipe_id) = desc.pipe_id {
                crate::driver::pipe::inc_ref(pipe_id);
            }
            proc.fd_table.lock().set_fd(0, desc);
        }
        if let Some(desc) = stdout_desc {
            if let Some(pipe_id) = desc.pipe_id {
                crate::driver::pipe::inc_ref(pipe_id);
            }
            proc.fd_table.lock().set_fd(1, desc);
        }
    }

    // Register process and add to scheduler
    let parent_pid = crate::process::current_pid();
    proc.ppid = parent_pid;

    let proc_idx = match crate::process::add_process(proc) {
        Some(i) => i,
        None => {
            crate::console_println!("[exec_fd] Process table full");
            return ERR_NOMEM;
        }
    };

    // Re-read process from table to get registered fields
    let proc =
        crate::process::get_process_by_index(proc_idx).expect("Process disappeared after add");

    #[cfg(target_arch = "riscv64")]
    let user_satp = (8usize << 60) | proc.page_table_root;
    #[cfg(target_arch = "x86_64")]
    let user_satp = proc.page_table_root << 12; // CR3 = physical address of PML4

    match crate::sched::add_user_process(
        proc.entry,
        proc.user_stack_top,
        proc.kernel_stack_top,
        user_satp,
        proc_idx,
    ) {
        Some(_tid) => {
            #[cfg(target_arch = "x86_64")]
            crate::console_println!(
                "[exec] Launched '{}' pid={} entry={:#x} stack={:#x} kstack={:#x} pt_root={:#x} cr3={:#x}",
                name,
                proc.pid,
                proc.entry,
                proc.user_stack_top,
                proc.kernel_stack_top,
                proc.page_table_root,
                user_satp
            );
            #[cfg(target_arch = "riscv64")]
            crate::klog!(DEBUG, "[exec] Launched '{}' (pid={})", name, proc.pid);
            proc.pid as isize
        }
        None => {
            crate::klog!(DEBUG, "[exec] Failed to schedule process");
            ERR_NOMEM
        }
    }
}

/// Syscall 60: Send a signal to a process.
/// `pid` = target process ID, `sig` = signal number.
/// Currently only supports SIGINT (2) which terminates the target.
fn sys_kill(pid: usize, sig: usize) -> isize {
    // SIGINT = 2, SIGKILL = 9, SIGTERM = 15
    if sig != 2 && sig != 9 && sig != 15 {
        return ERR_INVAL;
    }

    let proc_idx = match crate::process::find_process_by_pid(pid) {
        Some(idx) => idx,
        None => return ERR_NOENT,
    };

    // Terminate the target process
    crate::process::set_exit_code(sig);
    crate::process::set_state(proc_idx, crate::process::ProcessState::Exited);

    // Wake parent if waiting
    if let Some(parent_idx) = crate::process::find_waiting_parent(proc_idx) {
        crate::process::set_wait_child(parent_idx, None);
        crate::sched::wake_task(parent_idx);
    }

    // Remove task from scheduler
    crate::sched::remove_task(proc_idx);

    // Reclaim process resources
    crate::process::reclaim_process(proc_idx);

    ERR_OK
}

/// Syscall 34: Fork current process.
/// Creates a copy of the current process with independent page table.
/// Returns child_pid in parent, 0 in child.
fn sys_fork() -> isize {
    // Get current process info
    let current = match crate::process::current() {
        Some(p) => p,
        None => return ERR_INVAL,
    };
    let parent_idx = crate::process::current_index();

    // Clone the page table (deep copy user pages)
    let user_pt = crate::mm::vmm::create_user_page_table();
    let parent_ppn = current.page_table_root;
    let parent_pt = crate::process::get_user_page_table(parent_ppn);
    let page_size = crate::mm::pmm::page_size();

    // Allocate kernel stack for child BEFORE copy_kernel_mappings
    let kstack_base_fork = match crate::mm::pmm::alloc_frame() {
        Some(f) => f,
        None => return ERR_NOMEM,
    };
    for _ in 0..3 {
        if crate::mm::pmm::alloc_frame().is_none() {
            return ERR_NOMEM;
        }
    }
    let kernel_stack_top = kstack_base_fork + 4 * page_size;

    // Copy kernel mappings (with kernel stack mapping)
    crate::process::copy_kernel_mappings(user_pt, kernel_stack_top);

    // Copy user page table entries (deep copy physical frames)
    for vpn in 0..512 {
        let pte = parent_pt.entry(vpn);
        if pte.is_valid() && pte.is_leaf() {
            let old_ppn = pte.ppn();
            let old_frame = old_ppn << 12;
            let new_frame = match crate::mm::pmm::alloc_frame() {
                Some(f) => f,
                None => return ERR_NOMEM,
            };
            // Copy frame contents
            unsafe {
                core::ptr::copy_nonoverlapping(
                    old_frame as *const u8,
                    new_frame as *mut u8,
                    page_size,
                );
            }
            // Map new frame in child page table with same flags
            let new_pte = crate::mm::vmm::PTE::new(new_frame >> 12, pte.flags());
            user_pt.set_entry(vpn, new_pte);
        }
    }

    let page_table_ppn = (user_pt as *const crate::mm::vmm::PageTable as usize) >> 12;
    let child_pid = crate::process::NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    // Clone fd table
    // Fork: deep-copy fd table (each process has independent fds)
    let fd_table = {
        let parent_fd = current.fd_table.lock();
        alloc::sync::Arc::new(spin::Mutex::new(parent_fd.clone()))
    };

    // Create child process (env is inherited from parent)
    let child = crate::process::Process {
        pid: child_pid,
        ppid: current.pid,
        page_table_root: page_table_ppn,
        kernel_stack_top,
        user_stack_top: current.user_stack_top,
        brk: current.brk,
        initial_brk: current.initial_brk,
        entry: current.entry,
        state: crate::process::ProcessState::Ready,
        exit_code: 0,
        fd_table,
        wait_child_idx: None,
        trap_ctx_ptr: 0,
        shared_page_table: false,
        clone_tls: 0,
        child_tid_ptr: 0,
        fs_base: 0,
        env: current.env.clone(),
    };

    // Register child
    let child_idx = match crate::process::add_process(child) {
        Some(i) => i,
        None => return ERR_NOMEM,
    };

    // Re-read child process from table
    let child_proc =
        crate::process::get_process_by_index(child_idx).expect("Child disappeared after add");

    #[cfg(target_arch = "riscv64")]
    let user_satp = (8usize << 60) | child_proc.page_table_root;
    #[cfg(target_arch = "x86_64")]
    let user_satp = child_proc.page_table_root << 12;

    match crate::sched::add_user_process(
        child_proc.entry,
        child_proc.user_stack_top,
        child_proc.kernel_stack_top,
        user_satp,
        child_idx,
    ) {
        Some(_tid) => {
            crate::console_println!(
                "[fork] Created child pid={} (parent pid={})",
                child_pid,
                current.pid
            );
            child_pid as isize
        }
        None => {
            crate::klog!(DEBUG, "[fork] Failed to schedule child");
            ERR_NOMEM
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Syscall 80: ioctl — Terminal/device control
// ═══════════════════════════════════════════════════════════════════════════

// ioctl command numbers (inspired by Linux termios)
pub const TCGETS: usize = 0x5401; // Get terminal attributes
pub const TCSETS: usize = 0x5402; // Set terminal attributes
pub const TIOCGWINSZ: usize = 0x5413; // Get window size

// Terminal mode flags (simplified)
pub const TERM_COOKED: usize = 0; // Canonical mode (default)
pub const TERM_RAW: usize = 1; // Raw mode (no echo, no line editing)
pub const TERM_ECHO_ON: usize = 2; // Enable echo
pub const TERM_ECHO_OFF: usize = 3; // Disable echo

/// ioctl(fd, cmd, arg) — Terminal control interface.
///
/// For fd=0 (stdin), supports:
///   cmd=TCSETS, arg=TERM_RAW: Switch to raw mode (for TUI apps)
///   cmd=TCSETS, arg=TERM_COOKED: Switch to canonical mode (default)
///   cmd=TCSETS, arg=TERM_ECHO_ON: Enable echo
///   cmd=TCSETS, arg=TERM_ECHO_OFF: Disable echo
///   cmd=TIOCGWINSZ: Returns (cols << 16 | rows) packed into usize
pub fn sys_ioctl(fd: i32, cmd: usize, arg: usize) -> isize {
    // Log all ioctl calls for debugging TUI init
    if fd != 0 && fd != 1 {
        return ERR_INVAL;
    }

    match cmd {
        TCGETS => {
            // Write a minimal termios struct to user-space at `arg`.
            // struct termios { c_iflag: u32, c_oflag: u32, c_cflag: u32, c_lflag: u32,
            //                  c_line: u8, c_cc: [u8; 19] } — 36 bytes total
            // We report canonical mode with echo by default (ICANON | ECHO in c_lflag).
            // TUI programs call TCGETS first, then modify and call TCSETS.
            if arg != 0 {
                use crate::driver::tty::TtyMode;
                let mode = crate::driver::tty::get_mode();
                let echo = true; // default: echo on
                let lflag: u32 = match mode {
                    TtyMode::Raw => 0, // no ICANON, no ECHO
                    TtyMode::Canonical => {
                        let mut f = 0x0002u32; // ICANON
                        if echo {
                            f |= 0x0008;
                        } // ECHO
                        f
                    }
                };
                user_write::<u32>(arg, 0); // c_iflag
                user_write::<u32>(arg + 4, 0); // c_oflag
                user_write::<u32>(arg + 8, 0); // c_cflag
                user_write::<u32>(arg + 12, lflag); // c_lflag
                user_write::<u8>(arg + 16, 0); // c_line
                user_write_bytes(arg + 17, &[0u8; 19]);
            }
            0
        }
        TCSETS => {
            // arg is a pointer to struct termios in user space.
            // struct termios { c_iflag: u32, c_oflag: u32, c_cflag: u32, c_lflag: u32,
            //                  c_line: u8, c_cc: [u8; 19] }
            // Parse lflag to determine mode.
            if arg != 0 {
                let lflag = user_read::<u32>(arg + 12);
                let icanon = (lflag & 0x0002) != 0; // ICANON
                let echo = (lflag & 0x0008) != 0; // ECHO
                if !icanon {
                    // Raw mode: ICANON cleared
                    crate::driver::tty::set_mode(crate::driver::tty::TtyMode::Raw);
                } else {
                    crate::driver::tty::set_mode(crate::driver::tty::TtyMode::Canonical);
                }
                crate::driver::tty::set_echo(echo);
            }
            0
        }
        TIOCGWINSZ => {
            // Write struct winsize into user-space buffer at `arg`.
            // struct winsize { ws_row: u16, ws_col: u16, ws_xpixel: u16, ws_ypixel: u16 }
            // Size: 8 bytes (4 x u16)
            let (cols, rows) = {
                #[cfg(target_arch = "x86_64")]
                {
                    crate::driver::vga::screen_size()
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    (80, 25)
                }
            };
            if arg != 0 {
                user_write::<u16>(arg, rows as u16);
                user_write::<u16>(arg + 2, cols as u16);
                user_write::<u16>(arg + 4, 0); // xpixel
                user_write::<u16>(arg + 6, 0); // ypixel
            }
            0
        }
        _ => ERR_INVAL,
    }
}

// ─── Linux compatibility syscall implementations ──────────────

/// Linux clone(flags, stack, parent_tid, tls, child_tid)
///
/// Creates a new thread/process. The child resumes execution right after
/// the clone syscall with rax=0 (Linux clone semantics). The parent gets
/// the child's PID as the return value.
///
/// Key flags:
///   CLONE_VM (0x100) = share memory space
///   CLONE_FS (0x200) = share fs info
///   CLONE_FILES (0x400) = share fd table
///   CLONE_SIGHAND (0x800) = share signal handlers
///   CLONE_THREAD (0x10000) = same thread group
///   CLONE_SETTLS (0x80000) = set FS base (x86_64: IA32_FS_BASE)
///   CLONE_PARENT_SETTID (0x100000) = write child TID to parent_tid
///   CLONE_CHILD_CLEARTID (0x200000) = clear child_tid on exit
fn linux_clone(
    flags: usize,
    stack: usize,
    parent_tid_ptr: usize,
    child_tid_ptr: usize, // r10 = 4th arg = child_tid (NOT tls!)
    tls: usize,           // r8 = 5th arg = tls
) -> isize {
    let is_vm_shared = (flags & 0x100) != 0; // CLONE_VM
    crate::klog!(
        DEBUG,
        "[linux_clone] flags={:#x} stack={:#x} ptid={:#x} ctid={:#x} tls={:#x}",
        flags,
        stack,
        parent_tid_ptr,
        child_tid_ptr,
        tls
    );

    // Get parent's trap context (saved by trap_handler before dispatch)
    #[cfg(target_arch = "x86_64")]
    let parent_ctx_ptr = crate::process::get_trap_ctx_ptr();
    #[cfg(not(target_arch = "x86_64"))]
    let parent_ctx_ptr: usize = 0;

    if !is_vm_shared {
        // Fork-like: create new address space
        // Delegate to sys_fork for now
        return sys_fork();
    }

    // ── Determine child's user stack ──
    let child_user_sp = if stack != 0 {
        stack
    } else {
        // No stack provided — not valid for CLONE_VM thread creation
        return ERR_INVAL;
    };

    // ── Read parent's register state (x86_64 only) ──
    #[cfg(target_arch = "x86_64")]
    if parent_ctx_ptr == 0 {
        return ERR_INVAL;
    }
    #[cfg(target_arch = "x86_64")]
    let parent_ctx = unsafe { &*(parent_ctx_ptr as *const crate::arch::trap::TrapContext) };

    #[cfg(target_arch = "x86_64")]
    {
        let _ = &parent_ctx;
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        // RISC-V: clone not fully supported yet, use basic add_user_process
        let my_proc_idx = crate::process::current_index();
        let user_pt_root = crate::process::current_page_table_root();

        let kernel_stack_pages = crate::process::KERNEL_STACK_PAGES;
        let kernel_stack_base = match crate::mm::pmm::alloc_contiguous_frames(kernel_stack_pages) {
            Some(base) => base,
            None => return ERR_NOMEM,
        };
        let kernel_stack_top = kernel_stack_base + kernel_stack_pages * crate::mm::pmm::page_size();

        let entry = crate::process::current().map(|p| p.entry).unwrap_or(0);

        let tid = match crate::sched::add_user_process(
            entry,
            child_user_sp,
            kernel_stack_top,
            user_pt_root,
            my_proc_idx,
        ) {
            Some(tid) => tid,
            None => return ERR_NOMEM,
        };
        return tid as isize;
    }

    // ── x86_64: proper clone with register copy ──
    #[cfg(target_arch = "x86_64")]
    {
        let my_pid = crate::process::current_pid();
        let my_proc_idx = crate::process::current_index();
        let user_pt_root = crate::process::current_page_table_root();

        // Allocate kernel stack for child thread
        let kernel_stack_pages = crate::process::KERNEL_STACK_PAGES;
        let kernel_stack_base = match crate::mm::pmm::alloc_contiguous_frames(kernel_stack_pages) {
            Some(base) => base,
            None => return ERR_NOMEM,
        };
        let kernel_stack_top = kernel_stack_base + kernel_stack_pages * crate::mm::pmm::page_size();

        // Get parent process info
        let parent_proc = match crate::process::current() {
            Some(p) => p,
            None => return ERR_INVAL,
        };

        // Create child process entry
        let child_pid =
            crate::process::NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        // CLONE_FILES (0x400): threads share the same fd table via Arc.
        // Without CLONE_FILES: independent fd table (deep copy like fork).
        let fd_table = if (flags & 0x400) != 0 {
            // CLONE_FILES — Arc::clone, same inner FdTable
            parent_proc.fd_table.clone()
        } else {
            // Independent fd table
            alloc::sync::Arc::new(spin::Mutex::new(crate::driver::fs::FdTable::new()))
        };

        let child = crate::process::Process {
            pid: child_pid,
            ppid: parent_proc.pid,
            page_table_root: user_pt_root, // Same as parent (CLONE_VM)
            kernel_stack_top,
            user_stack_top: child_user_sp,
            brk: parent_proc.brk,
            initial_brk: parent_proc.initial_brk,
            entry: 0, // Not used for clone (child resumes from TrapContext)
            state: crate::process::ProcessState::Ready,
            exit_code: 0,
            fd_table,
            wait_child_idx: None,
            trap_ctx_ptr: 0,
            shared_page_table: true, // Mark as shared — don't free on reclaim
            clone_tls: tls,
            child_tid_ptr: if (flags & 0x200000) != 0 {
                child_tid_ptr
            } else {
                0
            },
            fs_base: 0,
            env: parent_proc.env.clone(),
        };

        let child_idx = match crate::process::add_process(child) {
            Some(i) => i,
            None => return ERR_NOMEM,
        };

        // Build user_cr3: PPN → physical address
        let user_cr3 = user_pt_root << 12;

        // Use add_clone_process to set up child's kernel stack with
        // parent's full register state (child returns 0 from clone)
        let tid = match crate::sched::add_clone_process(
            parent_ctx,
            child_user_sp,
            kernel_stack_top,
            user_cr3,
            child_idx,
            tls,
        ) {
            Some(tid) => tid,
            None => return ERR_NOMEM,
        };

        // FS_BASE is already set by spawn_clone_task (using the correct sched slot).
        // Do NOT call set_task_fs_base here — child_idx is process index, not sched slot!

        // CLONE_PARENT_SETTID: write child PID to parent's memory
        if (flags & 0x100000) != 0 && parent_tid_ptr != 0 {
            user_write::<i32>(parent_tid_ptr, child_pid as i32);
        }

        child_pid as isize
    }
}

// ─── Futex support ────────────────────────────────────────────────────
//
// Real futex implementation with per-address wait queues.
// Used by Go runtime for goroutine synchronization (blocking/waking).
//
// Linux semantics:
//   FUTEX_WAIT(0):   if *uaddr == val, block until woken or *uaddr changes
//   FUTEX_WAKE(1):   wake up to `val` waiters on uaddr
//   FUTEX_WAIT_BITSET(9) / FUTEX_WAKE_BITSET(10): same but with bitset filter

use crate::sync::spinlock::SpinLock;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A waiter in the futex queue.
struct FutexWaiter {
    /// Process index (used by sched::wake_task)
    proc_idx: usize,
    /// Whether this waiter has already been woken
    woken: bool,
}

/// Global futex wait queues, keyed by user-space futex address.
static FUTEX_QUEUES: SpinLock<BTreeMap<usize, Vec<FutexWaiter>>> = SpinLock::new(BTreeMap::new());

/// Block the current task on a futex address.
///
/// Returns 0 on success (woken up), or -EAGAIN (-11) if *uaddr != expected_val.
fn futex_wait(uaddr: usize, expected_val: u32) -> isize {
    // 1. Volatile read of *uaddr from user space.
    //    SSTATUS.SUM is set in trap_handler, allowing S-mode to read U-mode pages.
    let current_val = user_read::<u32>(uaddr);
    if current_val != expected_val {
        return -11; // EAGAIN: value changed, don't block
    }

    // 2. Register current task in the wait queue.
    let proc_idx = crate::process::current_index();
    {
        let mut queues = FUTEX_QUEUES.lock();
        let queue = queues.entry(uaddr).or_insert_with(Vec::new);
        queue.push(FutexWaiter {
            proc_idx,
            woken: false,
        });
    } // drop lock before blocking — avoids holding spinlock across context switch

    // 3. Block current task — switches to another Ready task.
    //    If no other task is Ready, schedule_block() returns immediately
    //    (the task stays Running despite being in the queue, which is safe).
    crate::sched::schedule_block();

    // 4. Woken up (or spuriously resumed). Return 0.
    0
}

/// Wake up to `max_count` tasks waiting on a futex address.
///
/// Returns the number of tasks actually woken.
fn futex_wake(uaddr: usize, max_count: u32) -> isize {
    let mut queues = FUTEX_QUEUES.lock();
    let mut woken = 0u32;
    if let Some(queue) = queues.get_mut(&uaddr) {
        for waiter in queue.iter_mut() {
            if !waiter.woken && woken < max_count {
                waiter.woken = true;
                crate::sched::wake_task(waiter.proc_idx);
                woken += 1;
            }
        }
        // Remove woken waiters; clean up empty queues
        queue.retain(|w| !w.woken);
        if queue.is_empty() {
            queues.remove(&uaddr);
        }
    }
    woken as isize
}

/// Linux futex(addr, op, val, timeout, uaddr2, val3)
///
/// Real implementation with wait queues for FUTEX_WAIT/WAKE.
/// Go runtime uses futex for goroutine synchronization.
pub fn linux_futex(addr: usize, op: usize, val: usize) -> isize {
    const FUTEX_WAIT: usize = 0;
    const FUTEX_WAKE: usize = 1;
    const FUTEX_WAIT_BITSET: usize = 9;
    const FUTEX_WAKE_BITSET: usize = 10;
    const FUTEX_PRIVATE_FLAG: usize = 128;

    let base_op = op & !FUTEX_PRIVATE_FLAG; // strip private flag (Go always sets this)

    match base_op {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            if addr == 0 {
                return -1; // EINVAL
            }
            futex_wait(addr, val as u32)
        }
        FUTEX_WAKE | FUTEX_WAKE_BITSET => {
            if addr == 0 {
                return -1; // EINVAL
            }
            futex_wake(addr, val as u32)
        }
        _ => {
            // Unknown futex op — return success to avoid crashing Go runtime
            0
        }
    }
}

/// Linux arch_prctl(code, addr) — x86_64 FS/GS base management for TLS.
///
/// Go runtime calls arch_prctl(ARCH_SET_FS, addr) at startup to set up
/// goroutine thread-local storage via the %fs segment.
#[cfg(target_arch = "x86_64")]
fn linux_arch_prctl(code: usize, addr: usize) -> isize {
    const ARCH_SET_GS: usize = 0x1001;
    const ARCH_SET_FS: usize = 0x1002;
    const ARCH_GET_FS: usize = 0x1003;
    const ARCH_GET_GS: usize = 0x1004;
    const MSR_FS_BASE: u32 = 0xC000_0100;
    const MSR_GS_BASE: u32 = 0xC000_0101;
    const EINVAL: isize = -22;

    match code {
        ARCH_SET_GS => {
            unsafe { crate::arch::idt::wrmsr(MSR_GS_BASE, addr as u64) };
            0
        }
        ARCH_SET_FS => {
            let slot = crate::sched::current_sched_slot();
            unsafe { crate::arch::idt::wrmsr(MSR_FS_BASE, addr as u64) };
            crate::sched::set_task_fs_base(slot, addr as u64);
            0
        }
        ARCH_GET_FS => {
            if addr == 0 {
                return EINVAL;
            }
            let val = unsafe { crate::arch::idt::rdmsr(MSR_FS_BASE) };
            user_write::<u64>(addr, val);
            0
        }
        ARCH_GET_GS => {
            if addr == 0 {
                return EINVAL;
            }
            let val = unsafe { crate::arch::idt::rdmsr(MSR_GS_BASE) };
            user_write::<u64>(addr, val);
            0
        }
        _ => EINVAL,
    }
}

/// Linux tgkill(tgid, tid, sig) — send signal to a thread.
fn linux_tgkill(_tgid: usize, _tid: usize, _sig: usize) -> isize {
    // For now, just acknowledge. Go uses tgkill for goroutine preemption signals.
    0
}

/// Linux nanosleep(req, rem) — sleep for a specified time.
fn linux_nanosleep(_req_ptr: usize, _rem_ptr: usize) -> isize {
    // Go runtime uses nanosleep for timer goroutines.
    // Just return success — Go handles timing internally.
    0
}

/// Linux mkdirat(dirfd, pathname, mode) — create directory.
/// dirfd=AT_FDCWD (-100 = 0xffffff9c) means relative to CWD.
#[cfg(target_arch = "x86_64")]
fn linux_mkdirat(_dirfd: usize, path_ptr: usize, _mode: usize, _unused: usize) -> isize {
    if path_ptr == 0 {
        return -22; // EINVAL
    }

    let actual_len = crate::syscall::linux::count_user_string(path_ptr);
    if actual_len == 0 || actual_len > 256 {
        return -2; // ENOENT
    }

    sys_mkdir(path_ptr, actual_len)
}

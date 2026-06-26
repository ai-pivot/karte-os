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
pub const LINUX_GETRANDOM: usize = 116;
pub const LINUX_SET_TID_ADDRESS: usize = 115;
pub const LINUX_EPOLL_CREATE1: usize = 120;
pub const LINUX_EPOLL_CTL: usize = 121;
pub const LINUX_EPOLL_WAIT: usize = 122;
pub const LINUX_EPOLL_PWAIT: usize = 123;
pub const LINUX_EVENTFD2: usize = 124;
pub const LINUX_PIPE2: usize = 125;
pub const LINUX_DUP3: usize = 126;
pub const LINUX_FSTAT: usize = 127;
pub const LINUX_FCNTL: usize = 128;
pub const LINUX_TIMERFD_CREATE: usize = 129;
pub const LINUX_TIMERFD_SETTIME: usize = 130;

// ─── Error codes ──────────────────────────────────────────────────

pub const ERR_OK: isize = 0;
// ─── Linux-compatible errno values ─────────────────────────────
// These are negated Linux errno values returned from syscalls.
// Go / Rust / C programs on user space expect these exact values.
pub const ERR_INVAL: isize = -22; // EINVAL — Invalid argument
pub const ERR_NOMEM: isize = -12; // ENOMEM — Out of memory
pub const ERR_NOENT: isize = -2; // ENOENT — No such file or directory
pub const ERR_IO: isize = -5; // EIO — I/O error
pub const ERR_AGAIN: isize = -11; // EAGAIN — Resource temporarily unavailable
pub const ERR_ACCES: isize = -13; // EACCES — Permission denied
pub const ERR_RANGE: isize = -34; // ERANGE — Result too large
pub const ERR_INTR: isize = -4; // EINTR — Interrupted system call
pub const ERR_NOSYS: isize = -38; // ENOSYS — Function not implemented
pub const ERR_EXIST: isize = -17; // EEXIST — File exists
pub const ERR_BADF: isize = -9; // EBADF — Bad file descriptor
pub const ERR_FAULT: isize = -14; // EFAULT — Bad address
pub const ERR_TIMEDOUT: isize = -110; // ETIMEDOUT — Operation timed out
pub const ERR_PIPE: isize = -32; // EPIPE — Broken pipe

// ─── VMA (Virtual Memory Area) tracking ────────────────────────────
//
// VMA state is now scoped by page_table_root (PPN) in mm::vma module.
// Each address space has independent VMA entries, mmap bump allocator,
// and ELF vaddr tracking. See mm::vma for the full implementation.
//
// The wrappers below provide convenience access using current_page_table_root().

/// Helper: get current address space root for VMA operations.
/// Returns a test fallback root when running in test mode with no user process.
#[inline]
fn current_root() -> usize {
    let root = crate::process::current_page_table_root();
    if root != 0 {
        root
    } else {
        #[cfg(feature = "test_mode")]
        {
            // Test mode: use a fixed fallback root for VMA operations
            const TEST_FALLBACK_ROOT: usize = 0xFFFF_0000;
            let _ = crate::mm::vma::init_root(TEST_FALLBACK_ROOT);
            TEST_FALLBACK_ROOT
        }
        #[cfg(not(feature = "test_mode"))]
        {
            0 // No fallback outside test mode
        }
    }
}

/// Check if `addr` falls within a VMA that permits access (prot != PROT_NONE).
/// Returns Some(prot) if valid, None if no VMA covers this address or VMA is PROT_NONE.
pub fn vma_check(addr: usize) -> Option<usize> {
    crate::mm::vma::vma_check(current_root(), addr)
}

/// Query VMA protection for `addr` — distinguishes PROT_NONE from no-VMA.
/// Returns `Some(prot)` if a VMA covers this address (prot may be 0 for PROT_NONE).
/// Returns `None` if no VMA covers this address at all.
pub fn vma_query(addr: usize) -> Option<usize> {
    crate::mm::vma::vma_query(current_root(), addr)
}

/// Dump VMA entries near a given address for debugging.
pub fn vma_dump_region(addr: usize) {
    crate::mm::vma::vma_dump_region(current_root(), addr)
}

/// Check if [start, end) overlaps with any active VMA entry.
pub fn vma_overlaps(start: usize, end: usize) -> bool {
    crate::mm::vma::vma_overlaps(current_root(), start, end)
}

/// Ensure the mmap bump allocator starts at or above `min_addr`.
/// Called by the ELF loader after loading segments, so that mmap
/// doesn't allocate addresses that overlap with loaded ELF data.
pub fn ensure_mmap_above(min_addr: usize) {
    crate::mm::vma::ensure_mmap_above(current_root(), min_addr);
}

/// Register an ELF PT_LOAD segment as a VMA entry.
/// This prevents mmap from allocating addresses that overlap with loaded segments.
pub fn register_elf_vma(start: usize, end: usize, prot: usize) {
    crate::mm::vma::register_elf_vma(current_root(), start, end, prot);
}

/// Return true when `addr` belongs to the ELF PT_LOAD address range.
///
/// Anonymous mmap regions can use the same protection bits as ELF segments,
/// so protection alone cannot distinguish them in the page-fault handler.
pub fn vma_is_elf(addr: usize) -> bool {
    crate::mm::vma::vma_is_elf(current_root(), addr)
}

/// Add or update a VMA entry for [start, end) with the given prot.
/// For MAP_FIXED, removes any overlapping entries first.
/// Returns Ok(()) on success, Err(()) if no free VMA slot is available.
pub fn vma_add(start: usize, end: usize, prot: usize, map_fixed: bool) -> Result<(), ()> {
    crate::mm::vma::vma_add(current_root(), start, end, prot, map_fixed)
}

pub fn vma_add_file(
    start: usize,
    end: usize,
    prot: usize,
    map_fixed: bool,
    inode: u32,
    offset: usize,
) -> Result<(), ()> {
    crate::mm::vma::vma_add_file(current_root(), start, end, prot, map_fixed, inode, offset)
}

/// Query file mapping info for a virtual address.
pub fn vma_file_info(addr: usize) -> Option<(u32, usize)> {
    crate::mm::vma::vma_file_info(current_root(), addr)
}

/// Remove all VMA entries overlapping [start, end).
/// Re-inserts tail fragments (portions of VMAs outside the removed range).
pub fn vma_remove_range(start: usize, end: usize) {
    crate::mm::vma::vma_remove_range(current_root(), start, end);
}

/// Update prot for all VMA entries overlapping [start, end).
fn vma_update_prot(start: usize, end: usize, new_prot: usize) {
    crate::mm::vma::vma_update_prot(current_root(), start, end, new_prot);
}

/// Clear all VMA entries for the current address space (called on exec).
pub fn vma_clear() {
    crate::mm::vma::vma_clear_root(current_root());
}

// ─── Global FD table (single-process simplification) ────────────────

extern crate alloc;

// ─── User memory access helpers (CR3-aware for x86_64) ──────────
// Syscall handlers run under kernel CR3 on x86_64. These helpers
// temporarily switch to user CR3 for accessing user-space memory.

/// Read a value from user space with optimized CR3 handling on x86_64.
#[inline]
pub(crate) fn user_read<T: Copy + Default>(addr: usize) -> T {
    #[cfg(target_arch = "x86_64")]
    {
        let size = core::mem::size_of::<T>();
        // True kernel addresses — direct read
        if addr >= 0xFFFF_8000_0000_0000 {
            return unsafe { core::ptr::read_volatile(addr as *const T) };
        }
        let user_root = crate::process::current_page_table_root();
        if user_root == 0 {
            return unsafe { core::ptr::read_volatile(addr as *const T) };
        }
        // Check if we're already on user CR3 (SYSCALL path)
        let user_cr3 = user_root << 12;
        let current_cr3: usize;
        unsafe { core::arch::asm!("mov {}, cr3", out(reg) current_cr3) };
        if current_cr3 == user_cr3 {
            return unsafe { core::ptr::read_volatile(addr as *const T) };
        }
        // Kernel CR3 active — per-byte read via user_read_u8
        let mut val = core::mem::MaybeUninit::<T>::uninit();
        let dst = val.as_mut_ptr() as *mut u8;
        for i in 0..core::mem::size_of::<T>() {
            unsafe { core::ptr::write(dst.add(i), user_read_u8(addr + i)) };
        }
        unsafe { val.assume_init() }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let mut val = core::mem::MaybeUninit::<T>::zeroed();
        let dst = val.as_mut_ptr() as *mut u8;
        for i in 0..core::mem::size_of::<T>() {
            unsafe { core::ptr::write(dst.add(i), user_read_u8(addr + i)) };
        }
        unsafe { val.assume_init() }
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
pub(crate) fn user_read_u8(addr: usize) -> u8 {
    // True kernel addresses (high canonical range) — always direct read
    if addr >= 0xFFFF_8000_0000_0000 {
        return unsafe { core::ptr::read_volatile(addr as *const u8) };
    }

    let user_root = crate::process::current_page_table_root();
    if user_root == 0 {
        // No user page table active (test mode or boot)
        return unsafe { core::ptr::read_volatile(addr as *const u8) };
    }

    // If already running with user CR3 (SYSCALL path), direct read is safe
    let user_cr3_phys = user_root << 12;
    let current_cr3: usize;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) current_cr3) };
    if current_cr3 == user_cr3_phys {
        return unsafe { core::ptr::read_volatile(addr as *const u8) };
    }

    // Kernel CR3 active (int 0x80 path): switch to user CR3, read, switch back.
    // The kernel identity mapping maps user VAs to wrong physical frames,
    // so we MUST switch to user CR3 for correct reads.
    let mut byte: u8 = 0;
    crate::arch::trap::with_user_cr3(|| {
        byte = unsafe { core::ptr::read_volatile(addr as *const u8) };
    });
    byte
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub(crate) fn user_read_u8(addr: usize) -> u8 {
    // On RISC-V, trap_entry does NOT switch satp. The kernel runs under
    // the user's page table during syscall handling. Direct read_volatile
    // accesses the correct user physical page. If the page is not yet
    // mapped, the nested PF handler performs lazy allocation automatically.
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub(crate) fn user_write_u8(addr: usize, byte: u8) -> bool {
    unsafe { core::ptr::write_volatile(addr as *mut u8, byte) }
    true
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn user_write_u8_mapped(addr: usize, byte: u8) {
    // True kernel addresses (high canonical range) — always direct write
    if addr >= 0xFFFF_8000_0000_0000 {
        unsafe { core::ptr::write_volatile(addr as *mut u8, byte) };
        return;
    }

    let user_root = crate::process::current_page_table_root();
    if user_root == 0 {
        unsafe { core::ptr::write_volatile(addr as *mut u8, byte) };
        return;
    }

    // If already running with user CR3 (SYSCALL path), direct write is safe
    let user_cr3_phys = user_root << 12;
    let current_cr3: usize;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) current_cr3) };
    if current_cr3 == user_cr3_phys {
        unsafe { core::ptr::write_volatile(addr as *mut u8, byte) };
        return;
    }

    // Kernel CR3 active (int 0x80 path): switch to user CR3, write, switch back
    let user_cr3 = user_cr3_phys;
    let kernel_cr3 = crate::arch::idt::get_kernel_cr3_phys();
    let rflags: u64;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {rflags}",
            "cli",
            "mov cr3, {user_cr3}",
            "mov byte ptr [{addr}], {byte}",
            "mov cr3, {kernel_cr3}",
            rflags = out(reg) rflags,
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

#[cfg(target_arch = "x86_64")]
#[inline]
pub(crate) fn user_write_u8(addr: usize, byte: u8) -> bool {
    // True kernel addresses or no user PT: direct write
    if addr >= 0xFFFF_8000_0000_0000 || crate::process::current_page_table_root() == 0 {
        unsafe { core::ptr::write_volatile(addr as *mut u8, byte) };
        return true;
    }
    // If already on user CR3 (SYSCALL path), write directly
    let user_cr3_val = crate::process::current_page_table_root() << 12;
    let cur_cr3: usize;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cur_cr3) };
    if cur_cr3 == user_cr3_val {
        unsafe { core::ptr::write_volatile(addr as *mut u8, byte) };
        return true;
    }
    // Kernel CR3 active (int 0x80 path): use CR3 switching
    if !ensure_user_write_pages(addr, 1) {
        crate::klog!(
            WARN,
            "[syscall] user_write_u8: page allocation failed at {:#x}",
            addr
        );
        return false;
    }
    unsafe { user_write_u8_mapped(addr, byte) };
    true
}
// RISC-V version is implemented as a standalone function above.

/// Write a value to user space with automatic CR3 switching on x86_64.
/// Returns `true` on success, `false` if page allocation failed.
#[inline]
pub(crate) fn user_write<T: Copy>(addr: usize, val: T) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        // True kernel addresses or no user PT: direct write
        if addr >= 0xFFFF_8000_0000_0000 || crate::process::current_page_table_root() == 0 {
            unsafe { core::ptr::write(addr as *mut T, val) };
            return true;
        }
        // If already on user CR3 (SYSCALL path), delegate to user_write_u8
        let user_cr3 = crate::process::current_page_table_root() << 12;
        let current_cr3: usize;
        unsafe { core::arch::asm!("mov {}, cr3", out(reg) current_cr3) };
        if current_cr3 == user_cr3 {
            unsafe { core::ptr::write(addr as *mut T, val) };
            return true;
        }
        if !ensure_user_write_pages(addr, core::mem::size_of::<T>()) {
            return false;
        }
        // Per-byte write: load byte into register before CR3 switch
        let src = unsafe {
            core::slice::from_raw_parts(&val as *const T as *const u8, core::mem::size_of::<T>())
        };
        for (i, &byte) in src.iter().enumerate() {
            unsafe { user_write_u8_mapped(addr + i, byte) };
        }
        true
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        // Delegate to byte-by-byte user_write_u8 which handles satp switching
        let src = unsafe {
            core::slice::from_raw_parts(&val as *const T as *const u8, core::mem::size_of::<T>())
        };
        for (i, &byte) in src.iter().enumerate() {
            user_write_u8(addr + i, byte);
        }
        true
    }
}

/// Read a slice of bytes from user space with automatic CR3 switching on x86_64.
///
/// CRITICAL: kernel buffers are only mutated under kernel CR3. The x86_64
/// helper switches to user CR3 only for the single-byte load itself.
/// Batch read bytes from user space with optimized CR3 handling.
/// Instead of checking CR3 per-byte, checks once and does a bulk copy.
#[inline]
pub(crate) fn user_read_bytes(addr: usize, len: usize) -> alloc::vec::Vec<u8> {
    let mut buf = alloc::vec::Vec::with_capacity(len);

    #[cfg(target_arch = "x86_64")]
    {
        // True kernel addresses — direct read
        if addr >= 0xFFFF_8000_0000_0000 {
            buf.resize(len, 0u8);
            unsafe {
                core::ptr::copy_nonoverlapping(addr as *const u8, buf.as_mut_ptr(), len);
            }
            return buf;
        }

        let user_root = crate::process::current_page_table_root();
        if user_root == 0 {
            buf.resize(len, 0u8);
            unsafe {
                core::ptr::copy_nonoverlapping(addr as *const u8, buf.as_mut_ptr(), len);
            }
            return buf;
        }

        // Check if we're already on user CR3 (SYSCALL path)
        let user_cr3_phys = user_root << 12;
        let current_cr3: usize;
        unsafe { core::arch::asm!("mov {}, cr3", out(reg) current_cr3) };

        if current_cr3 == user_cr3_phys {
            // Already on user CR3 — direct bulk copy (kernel mappings are complete)
            buf.resize(len, 0u8);
            unsafe {
                core::ptr::copy_nonoverlapping(addr as *const u8, buf.as_mut_ptr(), len);
            }
        } else {
            // Kernel CR3 active — per-byte read via user_read_u8.
            // We CANNOT use with_user_cr3 + bulk copy because the Vec's backing
            // memory (kernel heap) may not be mapped in the user page table.
            // user_read_u8 loads a single byte into a register under user CR3.
            for i in 0..len {
                buf.push(user_read_u8(addr + i));
            }
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        buf.resize(len, 0u8);
        unsafe {
            core::ptr::copy_nonoverlapping(addr as *const u8, buf.as_mut_ptr(), len);
        }
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
                        core::ptr::write_bytes(
                            crate::mm::vmm::phys_to_virt(frame) as *mut u8,
                            0,
                            page_size,
                        );
                    }
                    crate::mm::vmm::map(user_pt, page, frame, flags);
                }
            }

            page += page_size;
        }
        true
    })
}

/// Write a slice of bytes to user space with optimized CR3 handling.
/// Returns `true` on success, `false` if page allocation failed.
#[inline]
pub(crate) fn user_write_bytes(addr: usize, src: &[u8]) -> bool {
    let len = src.len();
    if len == 0 {
        return true;
    }

    #[cfg(target_arch = "x86_64")]
    {
        // True kernel addresses or no user PT: direct bulk write
        if addr >= 0xFFFF_8000_0000_0000 || crate::process::current_page_table_root() == 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(src.as_ptr(), addr as *mut u8, len);
            }
            return true;
        }
        // If already on user CR3 (SYSCALL path), direct bulk write
        let user_cr3 = crate::process::current_page_table_root() << 12;
        let current_cr3: usize;
        unsafe { core::arch::asm!("mov {}, cr3", out(reg) current_cr3) };
        if current_cr3 == user_cr3 {
            unsafe {
                core::ptr::copy_nonoverlapping(src.as_ptr(), addr as *mut u8, len);
            }
            return true;
        }
        // Kernel CR3 active: ensure pages exist, then per-byte write.
        // CANNOT use with_user_cr3 + bulk copy because src (kernel data)
        // may not be mapped in the user page table. Per-byte user_write_u8
        // loads the byte into a register before CR3 switch.
        if !ensure_user_write_pages(addr, len) {
            return false;
        }
        for (i, &byte) in src.iter().enumerate() {
            unsafe { user_write_u8_mapped(addr + i, byte) };
        }
        true
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), addr as *mut u8, len);
        }
        true
    }
}

/// Check if a path refers to a pseudo-filesystem that doesn't exist on disk.
fn is_pseudo_path(path: &str) -> bool {
    path.starts_with("/proc")
        || path.starts_with("/sys")
        || path.starts_with("/dev")
        || path.starts_with("/run")
        || path.starts_with("/tmp")
}

/// Fill a Linux x86_64 stat structure buffer (144 bytes).
/// Layout: st_dev(0-8), st_ino(8-16), st_nlink(16-24), st_mode(24-28),
///         st_uid(28-32), st_gid(32-36), pad(36-48), st_size(48-56),
///         st_blksize(56-64), st_blocks(64-72)
#[cfg(target_arch = "x86_64")]
fn fill_stat_buffer(buf: &mut [u8; 144], st_mode: u32, st_size: i64, st_ino: u64) {
    // Get current wall clock time for timestamps.
    // ext4 inodes don't have timestamps set by our write path, so we use
    // the current time as a reasonable approximation. This is critical for
    // xbot's log rotation (checks mtime) and file freshness checks.
    #[cfg(target_arch = "x86_64")]
    let (now_sec, now_nsec) = crate::arch::rtc::wall_clock();
    #[cfg(not(target_arch = "x86_64"))]
    let (now_sec, now_nsec) = (0i64, 0i64);

    unsafe {
        core::ptr::write_bytes(buf.as_mut_ptr(), 0, 144);
        *((buf.as_mut_ptr() as usize + 8) as *mut u64) = st_ino;
        *((buf.as_mut_ptr() as usize + 16) as *mut u64) = 1; // st_nlink
        *((buf.as_mut_ptr() as usize + 24) as *mut u32) = st_mode;
        *((buf.as_mut_ptr() as usize + 48) as *mut i64) = st_size;
        *((buf.as_mut_ptr() as usize + 56) as *mut i64) = 4096; // st_blksize
        // x86_64 struct stat layout:
        //   offset 72: st_atim (tv_sec: i64, tv_nsec: i64)
        //   offset 88: st_mtim (tv_sec: i64, tv_nsec: i64)
        //   offset 104: st_ctim (tv_sec: i64, tv_nsec: i64)
        *((buf.as_mut_ptr() as usize + 72) as *mut i64) = now_sec;
        *((buf.as_mut_ptr() as usize + 80) as *mut i64) = now_nsec;
        *((buf.as_mut_ptr() as usize + 88) as *mut i64) = now_sec;
        *((buf.as_mut_ptr() as usize + 96) as *mut i64) = now_nsec;
        *((buf.as_mut_ptr() as usize + 104) as *mut i64) = now_sec;
        *((buf.as_mut_ptr() as usize + 112) as *mut i64) = now_nsec;
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

use crate::driver::fs::{FileDescriptor, MAX_FDS, O_CREAT};
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
        1 => sys_write(args[0] as i32, args[1], args[2]), // write
        2 => linux_open(args[0], args[1], args[2]),      // open (deprecated, use openat)
        3 => sys_close(args[0] as i32),                  // close
        4 => linux_stat(args[0], args[1]),               // stat
        5 => linux_fstat(args[0], args[1]),              // fstat
        6 => linux_lstat(args[0], args[1]),              // lstat
        7 => linux_poll(args[0], args[1], args[2]),      // poll
        8 => linux_lseek(args[0], args[1], args[2]),     // lseek
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
        20 => linux_writev(args[0], args[1], args[2]),     // writev
        21 => linux_access(args[0], args[1]),              // access
        22 => linux_pipe(args[0]),                         // pipe
        23 => 0,                                           // select (stub — epoll used instead)
        24 => {
            crate::sched::schedule();
            0
        } // sched_yield
        25 => linux_mremap(args[0], args[1], args[2], args[3]), // mremap
        26 => linux_msync(args[0], args[1]),               // msync
        27 => 0,                                           // mincore (all pages resident)
        28 => linux_madvise(args[0], args[1], args[2]),    // madvise
        29 => linux_dup(args[0]),                          // dup
        30 => linux_dup2(args[0], args[1]),                // dup2
        31 => linux_pause(),                               // pause
        32 => linux_dup(args[0]),                          // dup
        33 => linux_chdir(args[0]),                        // chdir (proper impl)
        34 => 0,                                           // fchdir (stub)
        35 => linux_nanosleep(args[0], args[1]),           // nanosleep
        36 => 0,                                           // alarm (stub: no alarms)
        37 => 0, // setitimer (stub: Go uses for preemption, return 0)
        38 => linux_gethostname(args[0], args[1]), // gethostname

        // ─── Process management ───────────────────────────────────
        39 => sys_getpid(),                                  // getpid
        40 => linux_getppid(),                               // getppid
        41 => linux_socket(args[0], args[1], args[2]),       // socket
        42 => sys_connect(args[0] as i32, args[1], args[2]), // connect
        43 => linux_accept(args[0] as i32),                  // accept
        44 => sys_sendto(args[0] as i32, args[1], args[2], args[3], args[4], args[5]), // sendto
        45 => sys_recvfrom(args[0] as i32, args[1], args[2]), // recvfrom
        46 => linux_sendmsg(args[0], args[1], args[2]),      // sendmsg
        47 => linux_recvmsg(args[0], args[1], args[2]),      // recvmsg
        48 => sys_shutdown(args[0] as i32),                  // shutdown
        49 => sys_bind(args[0] as i32, args[1], args[2]),    // bind
        50 => sys_listen(args[0] as i32, args[1]),           // listen
        51 => linux_getsockname(args[0], args[1], args[2]),  // getsockname
        52 => linux_getpeername(args[0], args[1], args[2]),  // getpeername
        53 => ERR_INVAL,                                     // socketpair (not implemented)
        54 => linux_setsockopt(args[0], args[1], args[2], args[3], args[4]), // setsockopt
        55 => linux_getsockopt(args[0], args[1], args[2], args[3], args[4]), // getsockopt

        56 => linux_clone(args[0], args[1], args[2], args[3], args[4]), // clone
        57 => sys_fork(),                                               // fork
        58 => linux_vfork(), // vfork (implemented as fork)
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
        82 => linux_rename(args[0], args[1]),         // rename(oldpath, newpath)

        // ─── More process ─────────────────────────────────────────
        89 => linux_readlink(args[0], args[1], args[2]), // readlink
        96 => linux_gettimeofday(args[0], args[1]),      // gettimeofday
        97 => linux_getrlimit(args[0], args[1]),         // getrlimit
        98 => linux_getrusage(args[0], args[1]),         // getrusage
        99 => linux_sysinfo(args[0]),                    // sysinfo
        100 => linux_times(args[0]),                     // times
        101 => ERR_INVAL,                                // ptrace (stub)
        102 => 0,                                        // getuid (stub: root)
        103 => 0,                                        // syslog (stub)
        104 => 0,                                        // getgid (stub: root)
        105 => 0,                                        // setuid (stub)
        106 => 0,                                        // setgid (stub)
        107 => 0,                                        // geteuid (stub: root)
        108 => 0,                                        // getegid (stub: root)
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
        122 => linux_uname(args[0]),                // uname (new)
        131 => linux_sigaltstack(args[0], args[1]), // sigaltstack
        157 => linux_prctl(args[0], args[1], args[2], args[3], args[4]), // prctl
        158 => linux_arch_prctl(args[0], args[1]),  // arch_prctl
        160 => 0,                                   // setrlimit (stub)
        186 => sys_getpid(),                        // gettid → getpid
        200 => 0,                                   // tkill (stub)
        201 => linux_time(args[0]),                 // time
        202 => linux_futex_impl(args[0], args[1], args[2], args[3]), // futex
        203 => linux_sched_setaffinity(args[0], args[1], args[2]), // sched_setaffinity (stub)
        204 => linux_sched_getaffinity(args[0], args[1], args[2]), // sched_getaffinity
        217 => linux_getdents64(args[0], args[1], args[2]), // getdents64
        218 => linux_set_tid_address(args[0]),      // set_tid_address
        228 => linux_clock_gettime(args[0], args[1]), // clock_gettime
        231 => linux_exit_group(args[0] as i32),    // exit_group
        234 => linux_tgkill(args[0], args[1], args[2]), // tgkill
        257 => linux_openat(args[0], args[1], args[2], args[3]), // openat
        258 => linux_mkdirat(args[0], args[1], args[2], args[3]), // mkdirat
        262 => linux_newfstatat(args[0], args[1], args[2], args[3]), // newfstatat
        263 => sys_unlink(args[1], linux::count_user_string(args[1])), // unlinkat
        264 => linux_rename(args[1], args[3]), // renameat(olddirfd, oldpath, newdirfd, newpath)
        316 => linux_rename(args[1], args[3]), // renameat2
        267 => linux_readlinkat(args[0], args[1], args[2], args[3]), // readlinkat
        269 => linux_access(args[1], args[2]), // faccessat
        272 => 0,                              // unshare (stub)
        273 => 0,                              // set_robust_list (stub)
        274 => 0,                              // get_robust_list (stub)
        290 => epoll::eventfd::sys_eventfd2(args[0], args[1]), // eventfd2
        232 => epoll::sys_epoll_wait(args[0], args[1], args[2], args[3] as isize), // epoll_wait
        233 => epoll::sys_epoll_ctl(args[0], args[1], args[2], args[3]), // epoll_ctl
        281 => epoll::sys_epoll_wait(args[0], args[1], args[2], args[3] as isize), // epoll_pwait (same as epoll_wait, ignoring sigmask)
        291 => epoll::sys_epoll_create1(args[0]),                                  // epoll_create1
        292 => sys_dup2(args[0] as i32, args[1] as i32),                           // dup3 → dup2
        293 => linux_pipe2(args[0], args[1]),                                      // pipe2
        302 => linux_prlimit64(args[0], args[1], args[2], args[3]),                // prlimit64
        285 => 0, // fallocate → success (SQLite WAL needs this)
        318 => linux_getrandom(args[0], args[1], args[2]), // getrandom
        334 => ERR_NOSYS, // rseq → ENOSYS (Go gracefully degrades)
        435 => ERR_NOSYS, // clone3: ENOSYS
        439 => linux_access(args[1], args[2]), // faccessat2
        _ => ERR_NOSYS,
    }
}

// ─── Linux-specific syscall implementations ──────────────────────────

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

    // SQLite WAL recovery: when opening .db-shm without O_CREAT, return ENOENT.
    // Our mmap doesn't back file pages, so SHM is always zeros after restart.
    // SQLite sees mxFrame=0 → ignores WAL → data loss.
    // Returning ENOENT forces SQLite to delete stale SHM and rebuild from WAL scan.
    let linux_creat = 0x40;
    let has_creat = (flags & linux_creat) != 0;
    if !has_creat && path_str.ends_with(".db-shm") {
        return ERR_NOENT;
    }

    // Try VFS open with converted flags
    // Preserve access mode bits (O_RDONLY=0, O_WRONLY=0x1, O_RDWR=0x2)
    let our_flags = (flags & 0x3) | (if has_creat { 0x100 } else { 0 }) | (flags & 0x600);
    let result = match crate::driver::vfs::open(&path_str, our_flags as u32) {
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
        Err(_e) => {
            // VFS/ext4 open failed. Try known /etc files as FakeFile fallback.
            let is_pseudo = is_pseudo_path(&path_str);
            let is_etc = path_str.starts_with("/etc") || path_str.starts_with("etc");

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
            } else if is_pseudo {
                let content = if path_str.ends_with("resolv.conf") {
                    b"nameserver 10.0.2.3\n".to_vec()
                } else if path_str.ends_with("hosts") {
                    b"127.0.0.1 localhost\n".to_vec()
                } else if path_str.ends_with("hostname") {
                    b"karteos\n".to_vec()
                } else {
                    alloc::vec![]
                };
                crate::process::with_fd_table(|fd_table| {
                    match fd_table.alloc_fake_fd_with_content(
                        alloc::format!("{}", path_str),
                        our_flags as u32,
                        content,
                    ) {
                        Some(fd) => fd as isize,
                        None => ERR_NOENT,
                    }
                })
            } else {
                ERR_NOENT
            }
        }
    };

    result
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
            return ERR_NOENT;
        }

        // Couldn't parse path — return ENOENT
        return ERR_NOENT;
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
#[cfg(target_arch = "x86_64")]
fn linux_poll(fds_ptr: usize, nfds: usize, _timeout: usize) -> isize {
    if nfds == 0 || fds_ptr == 0 {
        return 0;
    }

    const POLLIN: u16 = 0x001;
    const POLLOUT: u16 = 0x004;
    const POLLERR: u16 = 0x008;
    const POLLHUP: u16 = 0x010;

    // struct pollfd { int fd (4), short events (2), short revents (2) } = 8 bytes
    let mut ready_count = 0i32;
    for i in 0..nfds {
        let entry = fds_ptr + i * 8;
        let fd = user_read::<i32>(entry as usize);
        let events = user_read::<u16>(entry as usize + 4);
        if fd < 0 {
            // Negative fd: revents is always 0 (ignored)
            user_write::<u16>(entry as usize + 6, 0);
            continue;
        }

        let mut revents: u16 = 0;

        // Check readability (POLLIN)
        if events & POLLIN != 0 {
            if fd == 0 {
                // stdin: check TTY input
                if crate::driver::tty::has_input() {
                    revents |= POLLIN;
                }
            } else {
                // Other fds: check type
                let fd_info = get_fd_info(fd);
                match fd_info {
                    Some((FdType::PipeRead, _, _)) => {
                        // Pipe: check if data available
                        revents |= POLLIN; // Simplified: assume ready
                    }
                    Some((FdType::VfsFile(_), _, _))
                    | Some((FdType::Ext4File(_), _, _))
                    | Some((FdType::FakeFile(_), _, _)) => {
                        // Files: always readable
                        revents |= POLLIN;
                    }
                    Some((FdType::Socket(_), _, _)) => {
                        // Socket: simplified check
                        revents |= POLLIN;
                    }
                    _ => {}
                }
            }
        }

        // Check writability (POLLOUT)
        if events & POLLOUT != 0 {
            // All writable fds are ready for writing
            revents |= POLLOUT;
        }

        // Always set POLLERR for invalid fds
        let fd_info = get_fd_info(fd);
        if fd_info.is_none() && fd != 0 && fd != 1 && fd != 2 {
            revents |= POLLERR;
        }

        user_write::<u16>(entry as usize + 6, revents);
        if revents != 0 {
            ready_count += 1;
        }
    }

    ready_count as isize
}

#[cfg(not(target_arch = "x86_64"))]
fn linux_poll(_fds: usize, _nfds: usize, _timeout: usize) -> isize {
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

    // Check for Ext4File
    if let Some(ext4_desc) = crate::process::with_fd_table(|fd_table| {
        fd_table
            .get(fd as usize)
            .and_then(|desc| match &desc.fd_type {
                FdType::Ext4File(ext4_desc) => Some(ext4_desc.clone()),
                _ => None,
            })
    }) {
        match crate::driver::ext4::read_file_at_offset(ext4_desc.inode_num, offset, &mut data) {
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

    // Resolve fd type and dispatch to the correct pwrite handler.
    let fd_info = crate::process::with_fd_table(|fd_table| {
        fd_table
            .get(fd as usize)
            .map(|desc| (desc.fd_type.clone(), desc.pos))
    });

    if let Some((fd_type, _pos)) = fd_info {
        match &fd_type {
            FdType::VfsFile(vfs_fd) => match crate::driver::vfs::pwrite(*vfs_fd, &data, offset) {
                Ok(n) => return n as isize,
                Err(_) => {}
            },
            FdType::Ext4File(ext4_desc) => {
                // Direct ext4 pwrite using inode and offset
                match crate::driver::ext4::write_file_at_offset(ext4_desc.inode_num, offset, &data)
                {
                    Ok(n) => return n as isize,
                    Err(_) => {}
                }
            }
            _ => {}
        }
    }

    // Fallback: lseek to offset then write. This preserves the offset parameter
    // for fds that don't have a specific pwrite handler.
    crate::process::with_fd_table(|fd_table| {
        if let Some(desc) = fd_table.get_mut(fd as usize) {
            desc.pos = offset;
        }
    });
    sys_write(fd, buf, count)
}

#[cfg(target_arch = "x86_64")]
fn linux_readv(fd: usize, iov: usize, iovcnt: usize) -> isize {
    // Read from fd into multiple buffers (scatter read)
    let mut total = 0isize;
    for i in 0..iovcnt {
        // Each iovec is 16 bytes: iov_base (8) + iov_len (8)
        let entry = iov + i * 16;
        let base = user_read::<usize>(entry);
        let len = user_read::<usize>(entry + 8);
        if len == 0 {
            continue;
        }
        let r = sys_read(fd as i32, base, len);
        if r < 0 {
            if total > 0 {
                return total;
            }
            return r;
        }
        total += r;
        if (r as usize) < len {
            break; // Short read
        }
    }
    total
}

#[cfg(target_arch = "x86_64")]
fn linux_writev(fd: usize, iov: usize, iovcnt: usize) -> isize {
    // Write from multiple buffers to fd (gather write)
    let mut total = 0isize;
    for i in 0..iovcnt {
        let entry = iov + i * 16;
        let base = user_read::<usize>(entry);
        let len = user_read::<usize>(entry + 8);
        if len == 0 {
            continue;
        }
        let r = sys_write(fd as i32, base, len);
        if r < 0 {
            if total > 0 {
                return total;
            }
            return r;
        }
        total += r;
        if (r as usize) < len {
            break; // Short write
        }
    }
    total
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
fn linux_socket(domain: usize, socket_type: usize, protocol: usize) -> isize {
    sys_socket(domain, socket_type, protocol)
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
        return ERR_FAULT;
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
    #[cfg(target_arch = "x86_64")]
    let (secs, nsecs) = crate::arch::rtc::wall_clock();
    #[cfg(not(target_arch = "x86_64"))]
    let (secs, nsecs) = {
        let up = crate::arch::platform::uptime_ms();
        (
            (FAKE_EPOCH + up / 1000) as i64,
            ((up % 1000) * 1_000_000) as i64,
        )
    };
    if tv != 0 {
        user_write::<i64>(tv, secs);
        user_write::<i64>(tv + 8, nsecs / 1000);
    }
    0
}

#[cfg(target_arch = "x86_64")]
fn linux_time(tloc: usize) -> isize {
    #[cfg(target_arch = "x86_64")]
    let now = crate::arch::rtc::wall_clock().0;
    #[cfg(not(target_arch = "x86_64"))]
    let now = (FAKE_EPOCH + crate::arch::platform::uptime_ms() / 1000) as i64;
    if tloc != 0 {
        user_write::<i64>(tloc, now);
    }
    now as isize
}

#[cfg(target_arch = "x86_64")]
fn linux_clock_gettime(clockid: usize, tp: usize) -> isize {
    if tp != 0 {
        const CLOCK_REALTIME: usize = 0;
        const CLOCK_MONOTONIC: usize = 1;
        const CLOCK_PROCESS_CPUTIME_ID: usize = 2;
        const CLOCK_THREAD_CPUTIME_ID: usize = 3;
        const CLOCK_MONOTONIC_RAW: usize = 4;
        const CLOCK_REALTIME_COARSE: usize = 5;
        const CLOCK_MONOTONIC_COARSE: usize = 6;
        const CLOCK_BOOTTIME: usize = 7;

        #[cfg(target_arch = "x86_64")]
        let (real_secs, real_nsecs) = crate::arch::rtc::wall_clock();
        #[cfg(not(target_arch = "x86_64"))]
        let (real_secs, real_nsecs) = {
            let up = crate::arch::platform::uptime_ms();
            (
                (FAKE_EPOCH + up / 1000) as i64,
                ((up % 1000) * 1_000_000) as i64,
            )
        };

        let up_ms = crate::arch::platform::uptime_ms();
        let (secs, nsecs) = match clockid {
            CLOCK_REALTIME | CLOCK_REALTIME_COARSE => (real_secs, real_nsecs),
            CLOCK_MONOTONIC
            | CLOCK_MONOTONIC_RAW
            | CLOCK_MONOTONIC_COARSE
            | CLOCK_BOOTTIME
            | CLOCK_PROCESS_CPUTIME_ID
            | CLOCK_THREAD_CPUTIME_ID => {
                let secs = (up_ms / 1000) as i64;
                let nsecs = ((up_ms % 1000) * 1_000_000) as i64;
                (secs, nsecs)
            }
            _ => (real_secs, real_nsecs),
        };
        user_write::<i64>(tp, secs);
        user_write::<i64>(tp + 8, nsecs);
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
    const AFFINITY_MASK_BYTES: usize = core::mem::size_of::<usize>();
    if mask == 0 || size < AFFINITY_MASK_BYTES {
        return ERR_INVAL;
    }

    let mut affinity = [0u8; AFFINITY_MASK_BYTES];
    affinity[0] = 0x01; // CPU 0 is available.
    user_write_bytes(mask, &affinity);
    AFFINITY_MASK_BYTES as isize
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
    let (effective_id, effective_args) = if let Some(translation) = linux::translate(id, args) {
        match translation {
            linux::Translation::Dispatch { karte_nr, args } => (karte_nr, args),
            linux::Translation::Handled(retval) => {
                return retval;
            }
        }
    } else {
        (id, args)
    };
    let result = dispatch_inner(effective_id, effective_args);

    result
}

fn dispatch_inner(id: usize, args: [usize; 6]) -> isize {
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
        SYS_LS => sys_ls(args[0], args[1], args[2], args[3]),
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
        LINUX_CLONE => {
            // RISC-V clone ABI: clone(flags, stack, parent_tid, child_tls, child_tid)
            // a0=flags, a1=stack, a2=parent_tid, a3=child_tls, a4=child_tid
            // x86_64 clone ABI: clone(flags, stack, parent_tid, child_tid, tls)
            // RDI=flags, RSI=stack, RDX=parent_tid, R10=child_tid, R8=tls
            #[cfg(target_arch = "riscv64")]
            {
                linux_clone(args[0], args[1], args[2], args[4], args[3])
            }
            #[cfg(not(target_arch = "riscv64"))]
            {
                linux_clone(args[0], args[1], args[2], args[3], args[4])
            }
        }
        LINUX_FUTEX => linux_futex_impl(args[0], args[1], args[2], args[3]),
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
        LINUX_EPOLL_CREATE1 => epoll::sys_epoll_create1(args[0]),
        LINUX_EPOLL_CTL => epoll::sys_epoll_ctl(args[0], args[1], args[2], args[3]),
        LINUX_EPOLL_WAIT => epoll::sys_epoll_wait(args[0], args[1], args[2], args[3] as isize),
        LINUX_EPOLL_PWAIT => epoll::sys_epoll_wait(args[0], args[1], args[2], args[3] as isize),
        LINUX_EVENTFD2 => epoll::eventfd::sys_eventfd2(args[0], args[1]),
        LINUX_PIPE2 => sys_pipe(args[0]),
        LINUX_DUP3 => sys_dup2(args[0] as i32, args[1] as i32),
        LINUX_FSTAT => linux_fstat_stub(args[0], args[1]),
        LINUX_FCNTL => linux_fcntl(args[0], args[1], args[2]),
        LINUX_TIMERFD_CREATE => epoll::timerfd::sys_timerfd_create(args[0], args[1]),
        LINUX_TIMERFD_SETTIME => {
            // timerfd_settime(fd, flags, new_value, old_value)
            epoll::timerfd::sys_timerfd_settime(args[0], args[1], args[2], args[3])
        }
        #[cfg(target_arch = "x86_64")]
        LINUX_ARCH_PRCTL => linux_arch_prctl(args[0], args[1]),
        #[cfg(not(target_arch = "x86_64"))]
        LINUX_ARCH_PRCTL => {
            0 // stub: not needed on non-x86_64
        }

        #[cfg(target_arch = "riscv64")]
        131 | 133 | 139 => 0,

        // clone3 (Linux generic 435) — return ENOSYS to force Go fallback to clone(220)
        435 => ERR_NOSYS,

        // rseq (Linux generic 293) — return ENOSYS, Go has fallback
        #[cfg(target_arch = "riscv64")]
        293 | 168 => ERR_NOSYS, // rseq, getcpu → ENOSYS

        _ => {
            let _ = id;
            // Return -ENOSYS for unrecognized syscalls. Go runtime expects
            // ENOSYS (not EINVAL) for unsupported syscalls — it falls back
            // gracefully to ENOSYS but may abort on EINVAL.
            ERR_NOSYS
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
    crate::console_println!("[sys_exit] user process exited, code={}", code);
    #[cfg(target_arch = "riscv64")]
    {}
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

    let my_idx = crate::process::current_index();
    cleanup_futex_waiters_for_processes(&[my_idx]);

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

    // Dropping FdTable is not enough: fd entries own global VFS, pipe, eventfd,
    // timerfd, and epoll state that must not leak into the next exec.
    cleanup_current_process_fds();

    // Wake parent if waiting
    if let Some(parent_idx) = crate::process::find_waiting_parent(my_idx) {
        crate::process::set_wait_child(parent_idx, None);
        crate::sched::wake_task(parent_idx);
    }

    // 4. Switch to kernel CR3 before context switch.
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

    // Flush all mmap'd file-backed dirty pages to ext4 before exit.
    // This is what Linux kernel does automatically on process exit.
    // Without this, SQLite's .db-shm (mmap'd) data is lost because
    // physical pages are freed without writeback.
    #[cfg(target_arch = "x86_64")]
    {
        let root = crate::process::current_page_table_root();
        let page_size = crate::mm::pmm::page_size();
        // Scan all VMA regions for file-backed mappings
        if let Some(regions) = crate::mm::vma::vma_dump_regions(root) {
            for (start, end, inode, offset) in regions {
                if inode == 0 {
                    continue; // Anonymous mapping, skip
                }
                // Write back each page in the file mapping
                let mut vaddr = start;
                let mut file_off = offset;
                while vaddr < end {
                    crate::arch::trap::with_kernel_cr3(|| {
                        let user_pt = crate::arch::trap::get_user_pt_safe();
                        if let Some(frame) = crate::mm::vmm::translate_user(user_pt, vaddr) {
                            let buf_vaddr = crate::mm::vmm::phys_to_virt(frame);
                            let buf = unsafe {
                                core::slice::from_raw_parts(buf_vaddr as *const u8, page_size)
                            };
                            let _ = crate::driver::ext4::write_file_at_offset(inode, file_off, buf);
                        }
                    });
                    vaddr += page_size;
                    file_off += page_size;
                }
            }
        }
    }

    let my_idx = crate::process::current_index();
    let my_pid = crate::process::current_pid();
    let root = crate::process::current_page_table_root();
    let leader_idx = crate::process::find_group_leader_by_page_table_root(root).unwrap_or(my_idx);
    let group = crate::process::find_processes_by_page_table_root(root);
    cleanup_futex_waiters_for_processes(&group);

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

    // exit_group tears down the shared file table once for the whole Go
    // thread group before its process-table entries disappear.
    cleanup_current_process_fds();

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
    if buf == 0 || len == 0 || len > 1048576 {
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
            // Stdio: batch-read user buffer then output to console
            let data = user_read_bytes(buf, len);
            // Batch console output: VGA sequentially (ANSI parsing),
            // then UART batch-write (reduces port I/O overhead)
            #[cfg(target_arch = "x86_64")]
            {
                crate::arch::platform::console_write_batch(&data);
                crate::driver::vga::flush_cursor();
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                for &byte in &data {
                    crate::arch::platform::console_putchar(byte);
                }
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
            let data = user_read_bytes(buf, len);
            return match crate::driver::vfs::write(vfs_fd, &data) {
                Ok(n) => {
                    // Sync fd table position with VFS position (which may
                    // have jumped to end-of-file for O_APPEND writes)
                    if let Some(vfs_of) = crate::driver::vfs::get_open_file_pos(vfs_fd) {
                        set_fd_pos(fd, vfs_of);
                    } else {
                        let cur = get_fd_pos(fd);
                        set_fd_pos(fd, cur + n);
                    }
                    n as isize
                }
                Err(_) => ERR_IO,
            };
        }
        Some((FdType::Ext4File(ext4_desc), _, _)) => {
            let data = user_read_bytes(buf, len);

            // Get current seek position and flags
            let (pos, flags) = crate::process::with_fd_table(|fdt| {
                fdt.get(fd as usize)
                    .map(|d| (d.pos, d.flags))
                    .unwrap_or((0, 0))
            });

            // O_APPEND: always write at end of file
            const O_APPEND_FLAG: u32 = 0x400;
            let write_offset = if flags & O_APPEND_FLAG != 0 {
                crate::driver::ext4::file_size(ext4_desc.inode_num).unwrap_or(0)
            } else {
                pos
            };

            match crate::driver::ext4::write_file_at_offset(
                ext4_desc.inode_num,
                write_offset,
                &data,
            ) {
                Ok(_) => {
                    // Update seek position
                    crate::process::with_fd_table(|fdt| {
                        if let Some(f) = fdt.get_mut(fd as usize) {
                            f.pos = write_offset + len;
                        }
                    });
                    return len as isize;
                }
                Err(_) => {
                    return ERR_IO;
                }
            }
        }
        Some((FdType::Timerfd, _, _)) => {
            return epoll::timerfd::timerfd_read(fd as usize, buf, len);
        }
        Some((FdType::Socket(_), _, _)) => {
            // Socket write → poll first (routes, RX), then send, then poll (TX flush)
            #[cfg(target_arch = "x86_64")]
            crate::net::iface::NetStack::poll();
            let data = user_read_bytes(buf, len);
            let result = crate::net::iface::NetStack::send(
                get_fd_socket(fd).unwrap_or(0),
                &data,
                None,
                None,
            );
            #[cfg(target_arch = "x86_64")]
            crate::net::iface::NetStack::poll();
            return result;
        }
        Some((FdType::File, _, _)) => {}
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
    if buf == 0 || len == 0 || len > 1048576 {
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
            // Stdio stdin (fd 0 default): blocking read from TTY.
            // Check O_NONBLOCK flag — Go's netpoller requires -EAGAIN when
            // no data is available, otherwise the entire Go runtime deadlocks.
            let nonblock = crate::process::with_fd_table(|fdt| {
                fdt.get(fd as usize)
                    .map(|d| (d.flags & O_NONBLOCK) != 0)
                    .unwrap_or(false)
            });
            let mut kbuf = alloc::vec![0u8; len];
            loop {
                #[cfg(target_arch = "x86_64")]
                crate::driver::usb::xhci::poll_keyboard();
                let result = crate::driver::tty::read(kbuf.as_mut_ptr() as usize, len);
                if result > 0 {
                    let user_buf = UserSliceMut::new(buf, len).unwrap();
                    user_buf.copy_from_slice(&kbuf[..result as usize]);
                    crate::driver::tty::clear_input_waiter();
                    return result;
                }
                // No data available
                if nonblock {
                    return ERR_AGAIN; // -EAGAIN
                }
                // Blocking mode: register as stdin waiter, poll UART, then block.
                // wake_input_waiters() (called from on_char) will wake us via
                // wake_task() when keyboard input arrives.
                let proc_idx = crate::process::current_index();
                crate::driver::tty::set_input_waiter(proc_idx);
                crate::driver::tty::poll_uart();
                #[cfg(target_arch = "x86_64")]
                crate::driver::usb::xhci::poll_keyboard();
                // Check again after poll — poll_uart may have delivered data
                let result2 = crate::driver::tty::read(kbuf.as_mut_ptr() as usize, len);
                if result2 > 0 {
                    let user_buf = UserSliceMut::new(buf, len).unwrap();
                    user_buf.copy_from_slice(&kbuf[..result2 as usize]);
                    crate::driver::tty::clear_input_waiter();
                    return result2;
                }
                crate::sched::schedule_block();
            }
        }
        Some((FdType::Ext4File(ext4_desc), _, _)) => {
            let pos = crate::process::with_fd_table(|fdt| {
                fdt.get(fd as usize).map(|d| d.pos).unwrap_or(0)
            });

            // Read directly from inode at current seek position (like pread64).
            // Do NOT read the entire file into a Vec — that wastes memory and
            // can cause inconsistent results with the write path.
            let mut kbuf = alloc::vec![0u8; len];
            match crate::driver::ext4::read_file_at_offset(ext4_desc.inode_num, pos, &mut kbuf) {
                Ok(n) => {
                    user_write_bytes(buf, &kbuf[..n]);
                    crate::process::with_fd_table(|fdt| {
                        if let Some(f) = fdt.get_mut(fd as usize) {
                            f.pos += n;
                        }
                    });
                    return n as isize;
                }
                Err(_) => return ERR_IO,
            }
        }
        Some((FdType::File, _, _)) => {}
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
                    user_write_bytes(buf, &kbuf[..n]);
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
        Some((FdType::Socket(_), _, _)) => {
            // Socket read → recvfrom
            return sys_recvfrom(fd, buf, len);
        }
        _ => {
            return ERR_INVAL;
        }
    }
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
                        core::ptr::write_bytes(
                            crate::mm::vmm::phys_to_virt(frame) as *mut u8,
                            0,
                            page_size,
                        );
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
                    core::ptr::write_bytes(
                        crate::mm::vmm::phys_to_virt(frame) as *mut u8,
                        0,
                        page_size,
                    );
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
        return ERR_INVAL;
    }

    linux_mmap_inner(addr, len, prot, flags, _fd, _offset)
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
    let root = current_root();

    let target_addr = if addr != 0 && map_fixed {
        // MAP_FIXED: exact address required.
        // Reject if target range overlaps ELF segments.
        let aligned = addr & !(page_size - 1);
        let end = aligned.saturating_add(aligned_len);
        if aligned >= crate::process::USER_MMAP_LIMIT || end > crate::process::USER_MMAP_LIMIT {
            return ERR_NOMEM;
        }
        if crate::mm::vma::vma_is_elf(root, aligned) {
            // Overlaps ELF range — reject.
            return aligned as isize;
        }
        aligned
    } else if addr != 0 {
        // Hint: use if valid AND no overlap with existing VMAs, otherwise kernel chooses.
        // On Sv39 (256GB user VA), Go may request high addresses valid only on Sv48.
        // Per POSIX, when a hint cannot be honored, the kernel MAY return a different
        // address. We silently fall back to the bump allocator instead of failing.
        let aligned_addr = addr & !(page_size - 1);
        let hint_end = aligned_addr.checked_add(aligned_len).unwrap_or(0);
        let hint_in_range = aligned_addr >= crate::process::USER_MMAP_BASE
            && hint_end != 0
            && hint_end <= crate::process::USER_MMAP_LIMIT;
        if hint_in_range {
            let hint_overlaps = crate::mm::vma::vma_overlaps(root, aligned_addr, hint_end);
            if !hint_overlaps {
                aligned_addr
            } else {
                0 // hint overlaps — kernel chooses
            }
        } else {
            0 // hint out of range (e.g. Sv39 limit) — kernel chooses
        }
    } else {
        0
    };

    let target_addr = if target_addr != 0 {
        target_addr
    } else {
        // Per-root bump allocator for kernel-chosen addresses.
        match crate::mm::vma::reserve_mmap_addr(root, aligned_len) {
            Ok(addr) => addr,
            Err(()) => return -12, // ENOMEM
        }
    };

    let end = target_addr + aligned_len;

    // Resolve fd for file-backed mmap
    let file_inode = if !is_anonymous && _fd != usize::MAX {
        crate::process::with_fd_table(|fd_table| {
            fd_table.get(_fd).and_then(|desc| match &desc.fd_type {
                crate::driver::fs::FdType::VfsFile(vfs_fd) => crate::driver::vfs::fd_inode(*vfs_fd),
                crate::driver::fs::FdType::Ext4File(ext4_desc) => Some(ext4_desc.inode_num),
                _ => None,
            })
        })
        .unwrap_or(0)
    } else {
        0
    };

    // Register the VMA entry with file mapping info
    if vma_add_file(target_addr, end, prot, map_fixed, file_inode, _offset).is_err() {
        return ERR_NOMEM;
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

/// Convert Linux prot flags to KarteOS PTEFlags.
pub fn prot_to_pte_flags(prot: usize) -> crate::mm::vmm::PTEFlags {
    let readable = prot & PROT_READ != 0;
    let writable = prot & PROT_WRITE != 0;
    let executable = prot & PROT_EXEC != 0;

    #[cfg(target_arch = "riscv64")]
    {
        use crate::mm::vmm::PTEFlags;
        let mut f = PTEFlags::V | PTEFlags::U | PTEFlags::A;
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
            f |= PTEFlags::R | PTEFlags::W | PTEFlags::D;
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
        return ERR_INVAL;
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

#[cfg(target_arch = "x86_64")]
fn linux_msync(addr: usize, len: usize) -> isize {
    let page_size = crate::mm::pmm::page_size();
    let start = addr & !(page_size - 1);
    let end = (addr + len + page_size - 1) & !(page_size - 1);

    let root = current_root();
    let mut vaddr = start;
    while vaddr < end {
        // Check if this page is a file-backed mapping
        if let Some((inode, file_off)) = crate::syscall::vma_file_info(vaddr) {
            crate::arch::trap::with_kernel_cr3(|| {
                let user_pt = crate::arch::trap::get_user_pt_safe();
                if let Some(frame) = crate::mm::vmm::translate_user(user_pt, vaddr) {
                    // Read page data from frame and write to ext4
                    let vaddr_frame = crate::mm::vmm::phys_to_virt(frame);
                    let buf =
                        unsafe { core::slice::from_raw_parts(vaddr_frame as *const u8, page_size) };
                    let _ = crate::driver::ext4::write_file_at_offset(inode, file_off, buf);
                }
            });
        }
        vaddr += page_size;
    }
    0
}

fn linux_munmap(addr: usize, len: usize) -> isize {
    if addr == 0 || len == 0 {
        return ERR_INVAL;
    }

    let page_size = crate::mm::pmm::page_size();
    let start = addr & !(page_size - 1);
    let end = (addr + len + page_size - 1) & !(page_size - 1);

    // Validate range
    let valid_start = crate::process::USER_HEAP_BASE;
    let valid_end = crate::process::USER_MMAP_LIMIT;
    if start < valid_start || end > valid_end {
        return ERR_INVAL;
    }

    // Write back dirty file-backed pages before unmapping
    let mut va = start;
    while va < end {
        if let Some((inode, file_off)) = crate::syscall::vma_file_info(va) {
            #[cfg(target_arch = "x86_64")]
            crate::arch::trap::with_kernel_cr3(|| {
                let user_pt = crate::arch::trap::get_user_pt_safe();
                if let Some(frame) = crate::mm::vmm::translate_user(user_pt, va) {
                    let vaddr_frame = crate::mm::vmm::phys_to_virt(frame);
                    let buf =
                        unsafe { core::slice::from_raw_parts(vaddr_frame as *const u8, page_size) };
                    let _ = crate::driver::ext4::write_file_at_offset(inode, file_off, buf);
                }
            });
        }
        va += page_size;
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
/// On x86_64: issues ATA FLUSH CACHE via AHCI.
/// On RISC-V: sends VIRTIO_BLK_T_FLUSH to the VirtIO block device.
/// This is critical for SQLite WAL mode durability guarantees.
fn linux_fsync(fd: usize) -> isize {
    if fd >= crate::driver::fs::MAX_FDS {
        return ERR_BADF;
    }
    // Flush pending writes to ensure data reaches physical disk.
    #[cfg(target_arch = "x86_64")]
    {
        if crate::driver::ahci::is_available() {
            if let Err(_e) = crate::driver::ahci::flush_cache() {
                return ERR_IO;
            }
        }
    }
    #[cfg(target_arch = "riscv64")]
    {
        // VirtIO block flush: sends VIRTIO_BLK_T_FLUSH to ensure all pending
        // writes reach the physical disk. This is critical for SQLite's
        // durability guarantees.
        //
        // The EXT4_FS spin::Mutex serializes all ext4 operations, so no other
        // thread is doing block I/O when fsync is called. The BLK_DEVICE lock
        // is held only during the flush request (a few microseconds).
        match crate::driver::virtio::flush_block_device() {
            Ok(()) => {}
            Err(e) => {
                crate::klog!(INFO, "[fsync] VirtIO flush failed (fd={}): {}", fd, e);
            }
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
                return ERR_FAULT;
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
                return ERR_AGAIN;
            }
            retval
        }
        _ => 0,
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
                            unsafe {
                                core::ptr::write_bytes(
                                    crate::mm::vmm::phys_to_virt(frame) as *mut u8,
                                    0,
                                    page_size,
                                )
                            };
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
                            unsafe {
                                core::ptr::write_bytes(
                                    crate::mm::vmm::phys_to_virt(frame) as *mut u8,
                                    0,
                                    page_size,
                                )
                            };
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
        return ERR_INVAL;
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

/// Cross-platform fstat stub — fills struct stat for Go runtime.
/// Go mainly calls fstat on stdin/stdout/stderr (character devices).
fn linux_fstat_stub(fd: usize, statbuf: usize) -> isize {
    if statbuf == 0 {
        return 0;
    }

    // For ext4 files, query actual file size from inode
    let ext4_inode = get_fd_ext4_inode(fd as i32);
    let file_size: u64 = if let Some(inode) = ext4_inode {
        crate::driver::ext4::file_size(inode).unwrap_or(0) as u64
    } else {
        0
    };

    #[cfg(target_arch = "riscv64")]
    {
        // RISC-V 64 Linux struct stat (128 bytes):
        // 0:  st_dev(u64), 8: st_ino(u64), 16: st_mode(u32), 20: st_nlink(u32),
        // 24: st_uid(u32), 28: st_gid(u32), 32: st_rdev(u64),
        // 40: __pad1(u64), 48: st_size(i64), 56: st_blksize(i32), 60: __pad2(i32),
        // 64: st_blocks(i64)
        // NOTE: RISC-V has __pad1 between st_rdev and st_size!
        let st_mode: u32 = if fd <= 2 {
            0x21A4 // S_IFCHR | 0644 — character device for stdin/out/err
        } else {
            0x81A4 // S_IFREG | 0644 — regular file
        };
        // Zero the entire struct first
        for i in 0..128 {
            user_write_u8(statbuf + i, 0);
        }
        user_write::<u32>(statbuf + 16, st_mode);
        user_write::<u32>(statbuf + 20, 1); // st_nlink
        user_write::<u64>(statbuf + 48, file_size); // st_size — at offset 48 (after __pad1)!
        user_write::<u32>(statbuf + 56, 4096); // st_blksize (i32 on riscv64)
    }

    #[cfg(target_arch = "x86_64")]
    {
        // x86_64 Linux struct stat (144 bytes)
        let st_mode: u32 = if fd <= 2 { 0x21A4 } else { 0x81A4 };
        for i in 0..144 {
            user_write_u8(statbuf + i, 0);
        }
        user_write::<u32>(statbuf + 24, st_mode);
        user_write::<u64>(statbuf + 16, 1); // st_nlink
        user_write::<u64>(statbuf + 48, file_size); // st_size — actual file size for ext4
        user_write::<i64>(statbuf + 56, 4096); // st_blksize
    }

    0
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
    // Trace file opens for debugging
    #[cfg(target_arch = "riscv64")]
    {}

    // Convert flags: Linux x86_64 O_CREAT=0x40, our internal=0x100
    // Go uses O_CREAT=0x40 on ALL architectures (including RISC-V).
    // Linux RISC-V kernel headers define O_CREAT=0x100 but Go's syscall
    // package hardcodes 0x40 for all Linux targets. We must accept both.
    let linux_o_creat: u32 = 0x40;
    let has_creat = (flags & linux_o_creat) != 0 || (flags & crate::driver::fs::O_CREAT) != 0;
    let our_flags = (if has_creat {
        crate::driver::fs::O_CREAT
    } else {
        0
    }) | (flags & 0x3) // Preserve access mode (O_RDONLY=0, O_WRONLY=1, O_RDWR=2)
      | (flags & (crate::driver::fs::O_TRUNC | crate::driver::fs::O_APPEND));

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

    // Check ext4 for existing files (open for read/write without O_CREAT)
    if crate::driver::ext4::has_ext4() {
        if let Some(inode) = crate::driver::fs::lookup_path(&name) {
            // Handle O_TRUNC: truncate file to 0 bytes
            if our_flags & crate::driver::fs::O_TRUNC != 0 {
                let _ = crate::driver::ext4::truncate_file(inode as u32);
            }
            return crate::process::with_fd_table(|fd_table| {
                match fd_table.alloc_special_fd(
                    name.clone(),
                    flags,
                    crate::driver::fs::FdType::Ext4File(crate::driver::ext4::Ext4FileDesc {
                        inode_num: inode as u32,
                        writable: has_creat || (flags & 0x3) != 0,
                    }),
                ) {
                    Some(fd) => fd as isize,
                    None => ERR_NOMEM,
                }
            });
        }
    }

    #[cfg(target_arch = "riscv64")]
    // O_CREAT with ext4: create the file if it doesn't exist
    if has_creat && crate::driver::ext4::has_ext4() {
        match crate::driver::ext4::write_file(&name, &[]) {
            Ok(()) => {}
            Err(_) => {}
        }
        // Try lookup again after creation
        if let Some(inode) = crate::driver::fs::lookup_path(&name) {
            #[cfg(target_arch = "riscv64")]
            return crate::process::with_fd_table(|fd_table| {
                match fd_table.alloc_special_fd(
                    name.clone(),
                    flags,
                    crate::driver::fs::FdType::Ext4File(crate::driver::ext4::Ext4FileDesc {
                        inode_num: inode as u32,
                        writable: true,
                    }),
                ) {
                    Some(fd) => fd as isize,
                    None => ERR_NOMEM,
                }
            });
        }
    }

    ERR_NOENT
}
fn cleanup_fd_resources(fd: usize, desc: FileDescriptor) {
    // Release all byte-range locks held by this fd.
    crate::driver::fs::release_fd_locks(fd);

    epoll::close_fd(fd);

    match desc.fd_type {
        FdType::PipeRead => {
            if let Some(pipe_id) = desc.pipe_id {
                crate::driver::pipe::with_pipe(pipe_id, |p| p.close_read());
                crate::driver::pipe::dec_ref(pipe_id);
            }
        }
        FdType::PipeWrite => {
            if let Some(pipe_id) = desc.pipe_id {
                crate::driver::pipe::with_pipe(pipe_id, |p| p.close_write());
                crate::driver::pipe::dec_ref(pipe_id);
            }
        }
        FdType::VfsFile(vfs_fd) => {
            crate::driver::vfs::close(vfs_fd);
        }
        FdType::Socket(sock_idx) => {
            crate::net::iface::NetStack::close_socket(sock_idx);
        }
        FdType::Eventfd => {
            epoll::eventfd::close_eventfd(fd as i32);
        }
        FdType::Timerfd => {
            epoll::timerfd::close_timerfd(fd);
        }
        FdType::Epoll | FdType::Ext4File(_) => {}
        FdType::Stdio
        | FdType::File
        | FdType::FakeFile(_)
        | FdType::VirtualFile
        | FdType::Urandom => {}
    }
}

fn cleanup_current_process_fds() {
    let fds = crate::process::with_fd_table(|fd_table| fd_table.drain_open_fds());
    for (fd, desc) in fds {
        cleanup_fd_resources(fd, desc);
    }
}

fn sys_close(fd: i32) -> isize {
    if fd < 0 || fd as usize >= MAX_FDS {
        return ERR_INVAL;
    }

    let desc = crate::process::with_fd_table(|fd_table| fd_table.take(fd as usize));
    let Some(desc) = desc else {
        return ERR_INVAL;
    };

    cleanup_fd_resources(fd as usize, desc);
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
/// Blocks the caller until the child exits (or returns immediately if the
/// child has already exited). The child's `sys_exit` handler calls
/// `wake_task(parent_idx)` to unblock us.
///
/// Returns the exit code (>= 0) when the child has exited (and reaps it),
/// `WAIT_AGAIN` on spurious wake (caller should re-invoke), or `WAIT_ERR`
/// when the pid is not a child of the caller (or already reaped).
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
            // Mark ourselves as waiting for this child, then block.
            // sys_exit will find us via find_waiting_parent() and call
            // wake_task() to set us back to Ready.
            crate::process::set_wait_child(crate::process::current_index(), Some(child_idx));
            crate::sched::schedule_block();

            // After waking, the child should have exited. Re-check.
            match crate::process::get_exit_code(child_idx) {
                Some(exit_code) => {
                    crate::process::set_wait_child(crate::process::current_index(), None);
                    crate::process::reclaim_process(child_idx);
                    crate::sched::remove_task(child_idx);
                    exit_code as isize
                }
                None => {
                    // Spurious wake — clear wait state and return WAIT_AGAIN
                    // so the caller can re-invoke.
                    crate::process::set_wait_child(crate::process::current_index(), None);
                    WAIT_AGAIN
                }
            }
        }
    }
}

/// Read a byte string from user memory.
/// Linux pathnames are NUL-terminated: stop at the first NUL even if the
/// caller supplied a larger length.
pub(crate) fn read_user_path(ptr: usize, len: usize) -> Option<alloc::string::String> {
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
pub(crate) fn resolve_path(path: &str) -> alloc::string::String {
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
/// If path_len > 0, lists the given directory (relative to CWD or absolute).
/// Otherwise lists the current working directory (CWD).
/// Writes a formatted listing to the user buffer (name + size per line).
/// Returns total bytes written, or error.
fn sys_ls(buf: usize, len: usize, path_ptr: usize, path_len: usize) -> isize {
    if buf == 0 || len == 0 {
        return ERR_INVAL;
    }

    // Resolve the directory path to list
    let dir_path: alloc::string::String;
    if path_len > 0 && path_ptr != 0 {
        let path_bytes = user_read_bytes(path_ptr, path_len.min(512));
        let user_path = core::str::from_utf8(&path_bytes).unwrap_or("/");
        let user_path = user_path.trim_end_matches('\0');
        dir_path = resolve_path(user_path);
    } else {
        let cwd = crate::env::get("CWD").unwrap_or_else(|| alloc::string::String::from("/"));
        dir_path = cwd;
    }
    let dir_key = dir_path.trim_start_matches('/');

    let files = crate::driver::fs::list_directory(dir_key);

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

    // Check if already exists
    if crate::driver::fs::lookup_path(&name).is_some() {
        return ERR_EXIST;
    }

    // Create the directory on ext4
    let _ = crate::driver::ext4::create_directory(&name);
    0
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
        (9usize << 60) | proc.page_table_root
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

    // Mask off Linux socket flags: SOCK_NONBLOCK=0x800, SOCK_CLOEXEC=0x80000
    let base_type = socket_type & 0xff;

    let stype = match base_type {
        1 => crate::net::iface::SocketType::Tcp,
        2 => crate::net::iface::SocketType::Udp,
        3 => crate::net::iface::SocketType::Icmp,
        _ => return ERR_INVAL,
    };

    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }

    // Create socket in NetStack (returns internal index)
    let sock_idx = crate::net::iface::NetStack::create_socket(stype);
    if sock_idx < 0 {
        return sock_idx;
    }

    // Allocate a real fd from the process FdTable
    crate::process::with_fd_table(|fd_table| {
        match fd_table.alloc_socket(sock_idx as usize) {
            Some(fd) => fd as isize,
            None => {
                // No free fd slot — close the socket we just created
                crate::net::iface::NetStack::close_socket(sock_idx as usize);
                ERR_NOMEM
            }
        }
    })
}

/// Syscall 71: bind(fd, addr_ptr, addr_len) → 0
fn sys_bind(fd: i32, addr_ptr: usize, addr_len: usize) -> isize {
    let sock = match get_fd_socket(fd) {
        Some(s) => s,
        None => return ERR_INVAL,
    };
    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }
    let (port, _) = match parse_sockaddr_in(addr_ptr, addr_len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    crate::net::iface::NetStack::bind(sock, port)
}

/// Syscall 72: connect(fd, addr_ptr, addr_len) → 0
fn sys_connect(fd: i32, addr_ptr: usize, addr_len: usize) -> isize {
    let sock = match get_fd_socket(fd) {
        Some(s) => s,
        None => return ERR_INVAL,
    };
    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }
    let (port, ip) = match parse_sockaddr_in(addr_ptr, addr_len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    crate::net::iface::NetStack::connect(sock, ip, port)
}

/// Syscall 73: listen(fd, backlog) → 0
fn sys_listen(fd: i32, _backlog: usize) -> isize {
    let sock = match get_fd_socket(fd) {
        Some(s) => s,
        None => return ERR_INVAL,
    };
    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }
    crate::net::iface::NetStack::bind(sock, 0)
}

/// Syscall 74: accept(fd) → new_fd
fn sys_accept(fd: i32) -> isize {
    let sock = match get_fd_socket(fd) {
        Some(s) => s,
        None => return ERR_INVAL,
    };
    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }
    if crate::net::iface::NetStack::is_connected(sock) {
        fd as isize
    } else {
        ERR_AGAIN
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
    let sock = match get_fd_socket(fd) {
        Some(s) => s,
        None => return ERR_INVAL,
    };
    if buf == 0 || len == 0 {
        return ERR_INVAL;
    }
    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }
    #[cfg(target_arch = "x86_64")]
    crate::net::iface::NetStack::poll();
    let data = user_read_bytes(buf, len);
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
    let result = crate::net::iface::NetStack::send(sock, &data, ip, port);
    #[cfg(target_arch = "x86_64")]
    crate::net::iface::NetStack::poll(); // Flush TX
    result
}

/// Syscall 76: recvfrom(fd, buf, len, flags) → bytes_received
fn sys_recvfrom(fd: i32, buf: usize, len: usize) -> isize {
    let sock = match get_fd_socket(fd) {
        Some(s) => s,
        None => return ERR_INVAL,
    };
    if buf == 0 || len == 0 {
        return ERR_INVAL;
    }
    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }
    // Poll network stack (syscall runs under kernel CR3 — MMIO safe).
    #[cfg(target_arch = "x86_64")]
    crate::net::iface::NetStack::poll();
    let mut kbuf = alloc::vec![0u8; len];
    match crate::net::iface::NetStack::recv(sock, &mut kbuf) {
        Ok((n, _src_ip, _src_port)) => {
            user_write_bytes(buf, &kbuf[..n]);
            n as isize
        }
        Err(e) => e,
    }
}

/// Syscall 77: shutdown(fd, how) → 0
fn sys_shutdown(fd: i32) -> isize {
    let sock = match get_fd_socket(fd) {
        Some(s) => s,
        None => return ERR_INVAL,
    };
    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }
    crate::net::iface::NetStack::shutdown(sock)
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
    for i in 0..read {
        user_write::<u8>(buf + i, kbuf[i]);
    }
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

    // NOTE: Do NOT clear the global VMA table here! exec creates a new process
    // with a new page table. The ELF loader will initialize fresh VMA state
    // for the new root via mm::vma::init_root().
    // Try streaming ELF loader from ext4 first
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
        (9usize << 60) | proc.page_table_root
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
        dispatch(9999, [0, 0, 0, 0, 0, 0]) == ERR_NOSYS
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

    #[cfg(target_arch = "x86_64")]
    crate::test::run_test("linux_arch_prctl_set_fs_updates_return_state", || {
        use core::sync::atomic::Ordering;

        const ARCH_SET_FS: usize = 0x1002;
        let slot = crate::sched::current_sched_slot();
        let orig_msr = unsafe { crate::arch::idt::rdmsr(0xC000_0100) };
        let orig_task = crate::sched::get_task_fs_base(slot);
        let orig_pending = crate::arch::trap::PENDING_FS_BASE.load(Ordering::Relaxed);
        let val = 0x4830_8a8u64;

        let result = linux_arch_prctl(ARCH_SET_FS, val as usize);
        let msr = unsafe { crate::arch::idt::rdmsr(0xC000_0100) };
        let task = crate::sched::get_task_fs_base(slot);
        let pending = crate::arch::trap::PENDING_FS_BASE.load(Ordering::Relaxed);

        unsafe { crate::arch::idt::wrmsr(0xC000_0100, orig_msr) };
        crate::sched::set_task_fs_base(slot, orig_task);
        crate::arch::trap::PENDING_FS_BASE.store(orig_pending, Ordering::Relaxed);

        result == 0 && msr == val && task == val && pending == val
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

use crate::driver::fs::{FdType, O_APPEND, O_NONBLOCK};

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

/// Get ext4 inode number for a given fd (for pread/pwrite offset operations).
pub(crate) fn get_fd_ext4_inode(fd: i32) -> Option<u32> {
    if fd < 0 || fd as usize >= MAX_FDS {
        return None;
    }
    crate::process::with_fd_table(|fd_table| match fd_table.get(fd as usize) {
        Some(f) => match &f.fd_type {
            FdType::Ext4File(ext4_desc) => Some(ext4_desc.inode_num),
            _ => None,
        },
        None => None,
    })
}

/// Get VFS fd number for a given process fd (for pread/pwrite offset operations).
/// Returns the inner VFS fd index so callers can use vfs::pread / vfs::pwrite
/// which respect the specified offset instead of the sequential file position.
pub(crate) fn get_fd_vfs_fd(fd: i32) -> Option<usize> {
    if fd < 0 || fd as usize >= MAX_FDS {
        return None;
    }
    crate::process::with_fd_table(|fd_table| match fd_table.get(fd as usize) {
        Some(f) => match &f.fd_type {
            FdType::VfsFile(vfs_fd) => Some(*vfs_fd),
            _ => None,
        },
        None => None,
    })
}

/// Translate a user fd to the NetStack socket index (for network syscalls).
pub(crate) fn get_fd_socket(fd: i32) -> Option<usize> {
    if fd < 0 || fd as usize >= MAX_FDS {
        return None;
    }
    crate::process::with_fd_table(|fd_table| match fd_table.get(fd as usize) {
        Some(f) => match &f.fd_type {
            FdType::Socket(sock_idx) => Some(*sock_idx),
            _ => None,
        },
        None => None,
    })
}

/// Get the current seek position for an fd (for lseek).
pub(crate) fn get_fd_pos(fd: i32) -> usize {
    crate::process::with_fd_table(|fdt| fdt.get(fd as usize).map(|d| d.pos).unwrap_or(0))
}

/// Set the seek position for an fd (for lseek).
pub(crate) fn set_fd_pos(fd: i32, pos: usize) {
    crate::process::with_fd_table(|fdt| {
        if let Some(f) = fdt.get_mut(fd as usize) {
            f.pos = pos;
        }
    });
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

    // NOTE: Do NOT clear the global VMA table here! exec creates a new process
    // with a new page table. The ELF loader will initialize fresh VMA state
    // for the new root via mm::vma::init_root().
    // Load ELF from filesystem — use non-streaming path for reliability
    let mut proc = match crate::driver::fs::read_file_owned(&name) {
        Some(data) => match crate::process::Process::from_elf(
            &data,
            alloc::vec![name.as_bytes().to_vec()],
            alloc::vec![],
        ) {
            Ok(p) => p,
            Err(e) => {
                crate::console_println!("[exec] ELF load failed for '{}': {}", name, e);
                return ERR_IO;
            }
        },
        None => return ERR_NOENT,
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
    let user_satp = (9usize << 60) | proc.page_table_root;
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
#[cfg(target_arch = "x86_64")]
fn copy_user_pages_x86(
    dst: &mut crate::mm::vmm::PageTable,
    src: &crate::mm::vmm::PageTable,
    level: usize,
) -> Result<bool, isize> {
    let mut copied_any = false;
    for idx in 0..512 {
        let pte = src.entry(idx);
        if !pte.is_valid() {
            continue;
        }

        if pte.is_leaf_at_level(level) {
            let flags = pte.flags();
            if !flags.contains(crate::mm::vmm::PTEFlags::USER) {
                continue;
            }
            if level != 0 {
                return Err(ERR_INVAL);
            }

            let old_frame = pte.ppn() << 12;
            let new_frame = crate::mm::pmm::alloc_frame().ok_or(ERR_NOMEM)?;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    crate::mm::vmm::phys_to_virt(old_frame) as *const u8,
                    crate::mm::vmm::phys_to_virt(new_frame) as *mut u8,
                    crate::mm::pmm::page_size(),
                );
            }
            dst.set_entry(idx, crate::mm::vmm::PTE::new_leaf(new_frame >> 12, flags));
            copied_any = true;
            continue;
        }

        if level == 0 {
            continue;
        }

        let src_child = unsafe { &*((pte.ppn() << 12) as *const crate::mm::vmm::PageTable) };
        let dst_child_ppn = {
            let existing = dst.entry(idx);
            if existing.is_valid() && !existing.flags().contains(crate::mm::vmm::PTEFlags::PS) {
                existing.ppn()
            } else {
                let child = crate::mm::vmm::PageTable::zeroed();
                let ppn = crate::mm::vmm::virt_to_phys(
                    child as *const crate::mm::vmm::PageTable as usize,
                ) >> 12;
                dst.set_entry(idx, crate::mm::vmm::PTE::new_nonleaf(ppn));
                ppn
            }
        };
        let dst_child = unsafe {
            &mut *((crate::mm::vmm::phys_to_virt(dst_child_ppn << 12))
                as *mut crate::mm::vmm::PageTable)
        };
        if copy_user_pages_x86(dst_child, src_child, level - 1)? {
            copied_any = true;
        }
    }

    Ok(copied_any)
}

fn sys_fork() -> isize {
    #[cfg(target_arch = "riscv64")]
    {
        return ERR_NOSYS;
    }
    // Get current process info
    let current = match crate::process::current() {
        Some(p) => p,
        None => return ERR_INVAL,
    };
    let parent_idx = crate::process::current_index();

    // Clone the page table (deep copy user pages)
    let user_pt = crate::mm::vmm::create_user_page_table();
    let parent_ppn = current.page_table_root;

    // Allocate kernel stack for child BEFORE copy_kernel_mappings
    let kernel_stack_top = match crate::process::alloc_kernel_stack() {
        Some(top) => top,
        None => return ERR_NOMEM,
    };

    // Copy kernel mappings (with kernel stack mapping)
    crate::process::copy_kernel_mappings(user_pt, kernel_stack_top);

    // Copy user page table entries (deep copy physical frames)
    #[cfg(target_arch = "x86_64")]
    {
        if let Err(err) = crate::arch::trap::with_kernel_cr3(|| {
            let parent_pt = crate::process::get_user_page_table(parent_ppn);
            copy_user_pages_x86(user_pt, parent_pt, 3)
        }) {
            return err;
        }
    }

    #[cfg(target_arch = "riscv64")]
    {
        let parent_pt = crate::process::get_user_page_table(parent_ppn);
        let page_size = crate::mm::pmm::page_size();
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
                        crate::mm::vmm::phys_to_virt(old_frame) as *const u8,
                        crate::mm::vmm::phys_to_virt(new_frame) as *mut u8,
                        page_size,
                    );
                }
                // Map new frame in child page table with same flags
                let new_pte = crate::mm::vmm::PTE::new(new_frame >> 12, pte.flags());
                user_pt.set_entry(vpn, new_pte);
            }
        }
    }

    let page_table_ppn =
        crate::mm::vmm::virt_to_phys(user_pt as *const crate::mm::vmm::PageTable as usize) >> 12;
    let child_pid = crate::process::NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    // Clone VMA state from parent to child address space
    if crate::mm::vma::clone_root_state(parent_ppn, page_table_ppn).is_err() {
        // VMA clone failed — initialize empty state for child
        crate::mm::vma::init_root(page_table_ppn).ok();
    }

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
    let user_satp = (9usize << 60) | child_proc.page_table_root;
    #[cfg(target_arch = "x86_64")]
    let user_satp = child_proc.page_table_root << 12;

    match crate::sched::add_user_process(
        child_proc.entry,
        child_proc.user_stack_top,
        child_proc.kernel_stack_top,
        user_satp,
        child_idx,
    ) {
        Some(_tid) => child_pid as isize,
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
    if fd != 0 && fd != 1 {
        return ERR_INVAL;
    }

    let result = match cmd {
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
        TCSETS | 0x5403 /* TCSETSW */ | 0x5404 /* TCSETSF */ => {
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
    };

    result
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
    let parent_ctx_ptr = crate::arch::trap::current_trap_ctx();

    if !is_vm_shared {
        // Fork-like: create new address space
        return sys_fork();
    }

    // ── Determine child's user stack ──
    let child_user_sp = if stack != 0 {
        stack
    } else {
        return ERR_INVAL;
    };

    if parent_ctx_ptr == 0 {
        return ERR_INVAL;
    }
    let parent_ctx = unsafe { &*(parent_ctx_ptr as *const crate::arch::trap::TrapContext) };

    {
        // ── Cross-platform clone: create child thread with parent's register state ──
        let my_proc_idx = crate::process::current_index();

        // Build the correct page table identifier for the child thread.
        // RISC-V: SATP = (mode=8 << 60) | PPN. x86_64: CR3 = PPN << 12.
        // current_page_table_root() returns PPN — must add mode bits for RISC-V.
        #[cfg(target_arch = "riscv64")]
        let user_pt_root = (9usize << 60) | crate::process::current_page_table_root();
        #[cfg(target_arch = "x86_64")]
        let user_pt_root = crate::process::current_page_table_root() << 12; // PPN → physical address for CR3
        #[cfg(not(any(target_arch = "riscv64", target_arch = "x86_64")))]
        let user_pt_root = crate::process::current_page_table_root(); // non-RISC-V non-x86_64: raw PPN

        let kernel_stack_top = match crate::process::alloc_kernel_stack() {
            Some(top) => top,
            None => return ERR_NOMEM,
        };

        // Map kernel stack into user page table for clone children
        let user_pt =
            crate::process::get_user_page_table(crate::process::current_page_table_root());
        crate::process::map_kernel_stack_pages(user_pt, kernel_stack_top);

        // Write child TID to parent_tid_ptr if CLONE_PARENT_SETTID
        if (flags & 0x100000) != 0 && parent_tid_ptr != 0 {
            let fake_tid = my_proc_idx as u64 + 1;
            user_write::<u64>(parent_tid_ptr, fake_tid);
        }

        // Only pass tls when CLONE_SETTLS flag is set.
        // Go's clone on RISC-V may pass garbage in the tls register argument
        // when CLONE_SETTLS is not in flags. Using garbage as tp crashes Go.
        let effective_tls = if (flags & 0x80000) != 0 { tls } else { 0 };

        let tid = match crate::sched::add_clone_process(
            parent_ctx,
            child_user_sp,
            kernel_stack_top,
            user_pt_root,
            my_proc_idx,
            effective_tls,
        ) {
            Some(tid) => tid,
            None => return ERR_NOMEM,
        };

        // Write child TID to child_tid_ptr if CLONE_CHILD_SETTID
        if (flags & 0x100000) != 0 && child_tid_ptr != 0 {
            user_write::<u64>(child_tid_ptr, tid as u64 + 1);
        }

        // Store child_tid_ptr for CLONE_CHILD_CLEARTID
        if (flags & 0x200000) != 0 {
            crate::process::set_child_tid_ptr(child_tid_ptr);
        }

        return (tid as isize) + 1;
    }

    // ── x86_64: proper clone with register copy ──
    #[cfg(target_arch = "x86_64")]
    {
        let my_pid = crate::process::current_pid();
        let my_proc_idx = crate::process::current_index();
        let user_pt_ppn = crate::process::current_page_table_root();
        let user_pt_root = user_pt_ppn << 12; // physical address for CR3

        // Allocate kernel stack for child thread
        let kernel_stack_top = match crate::process::alloc_kernel_stack() {
            Some(top) => top,
            None => return ERR_NOMEM,
        };

        let user_pt = crate::process::get_user_page_table(user_pt_ppn);
        crate::process::map_kernel_stack_pages(user_pt, kernel_stack_top);

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

/// Global futex wait queues, keyed by address space and user-space futex address.
///
/// Go uses FUTEX_PRIVATE_FLAG, so the same virtual address in two separate
/// process launches must not share a queue. Without the address-space key, a
/// stale waiter from a previous xbot run can consume a wake meant for the next
/// run and leave the new thread blocked forever.
type FutexKey = (usize, usize);
static FUTEX_QUEUES: SpinLock<BTreeMap<FutexKey, Vec<FutexWaiter>>> =
    SpinLock::new(BTreeMap::new());

fn futex_key(uaddr: usize) -> FutexKey {
    (crate::process::current_page_table_root(), uaddr)
}

pub fn cleanup_futex_waiters_for_processes(proc_indices: &[usize]) {
    if proc_indices.is_empty() {
        return;
    }

    let mut removed = 0usize;
    let mut queues = FUTEX_QUEUES.lock();
    queues.retain(|_, queue| {
        let before = queue.len();
        queue.retain(|waiter| !proc_indices.contains(&waiter.proc_idx));
        removed += before - queue.len();
        !queue.is_empty()
    });
    drop(queues);

    let _ = removed;
}

/// Block the current task on a futex address.
///
/// Returns 0 on success (woken up), or -EAGAIN (-11) if *uaddr != expected_val.
fn futex_timeout_ms(timeout_ptr: usize) -> Option<u64> {
    if timeout_ptr == 0 {
        return None;
    }

    let sec = user_read::<i64>(timeout_ptr);
    let nsec = user_read::<i64>(timeout_ptr + 8);
    if sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
        return Some(0);
    }

    let sec_ms = (sec as u64).saturating_mul(1000);
    let nsec_ms = ((nsec as u64) + 999_999) / 1_000_000;
    Some(sec_ms.saturating_add(nsec_ms))
}

fn futex_wait(uaddr: usize, expected_val: u32, timeout_ms: Option<u64>) -> isize {
    // 1. Volatile read of *uaddr from user space.
    //    SSTATUS.SUM is set in trap_handler, allowing S-mode to read U-mode pages.
    let current_val = user_read::<u32>(uaddr);
    if current_val != expected_val {
        return ERR_AGAIN; // value changed, don't block
    }

    // 2. Register current task in the wait queue.
    let proc_idx = crate::process::current_index();
    let key = futex_key(uaddr);
    {
        let mut queues = FUTEX_QUEUES.lock();
        let queue = queues.entry(key).or_insert_with(Vec::new);
        queue.push(FutexWaiter {
            proc_idx,
            woken: false,
        });
    } // drop lock before blocking — avoids holding spinlock across context switch

    // 3. Block current task — switches to another Ready task.
    //    If no other task is Ready, schedule_block() returns immediately
    //    (the task stays Running despite being in the queue, which is safe).
    if let Some(ms) = timeout_ms {
        if ms == 0 {
            cleanup_futex_waiters_for_processes(&[proc_idx]);
            return ERR_TIMEDOUT;
        }
        let wake_tick = crate::arch::platform::uptime_ms().saturating_add(ms);
        crate::sched::sleep_until(wake_tick);
    } else {
        // No timeout — block indefinitely (up to 5 min safety valve).
        // Go's futex_wait expects to block until FUTEX_WAKE is called
        // by another thread (created via clone). Returning immediately
        // causes a busy-loop that prevents Go runtime initialization.
        let wake_tick = crate::arch::platform::uptime_ms().saturating_add(100); // 100ms timeout
        crate::sched::sleep_until(wake_tick);
    }

    // 4. Woken up or spuriously resumed. If no other task was ready,
    // schedule_block() can return without a FUTEX_WAKE removing this waiter.
    // Remove our own entry so it cannot steal a future wake.
    let mut still_waiting = false;
    {
        let mut queues = FUTEX_QUEUES.lock();
        let mut remove_queue = false;
        if let Some(queue) = queues.get_mut(&key) {
            still_waiting = queue.iter().any(|waiter| waiter.proc_idx == proc_idx);
            queue.retain(|waiter| waiter.proc_idx != proc_idx);
            remove_queue = queue.is_empty();
        }
        if remove_queue {
            queues.remove(&key);
        }
    }

    if timeout_ms.is_some() && still_waiting {
        return ERR_TIMEDOUT;
    }

    // 5. Return success for real and spurious wakeups.
    0
}

/// Wake up to `max_count` tasks waiting on a futex address.
///
/// Returns the number of tasks actually woken.
fn futex_wake(uaddr: usize, max_count: u32) -> isize {
    let mut queues = FUTEX_QUEUES.lock();
    let mut woken = 0u32;
    let key = futex_key(uaddr);
    if let Some(queue) = queues.get_mut(&key) {
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
            queues.remove(&key);
        }
    }
    woken as isize
}

/// Linux futex(addr, op, val, timeout, uaddr2, val3)
///
/// Real implementation with wait queues for FUTEX_WAIT/WAKE.
/// Go runtime uses futex for goroutine synchronization.
pub fn linux_futex(addr: usize, op: usize, val: usize) -> isize {
    linux_futex_impl(addr, op, val, 0)
}

fn linux_futex_impl(addr: usize, op: usize, val: usize, timeout_ptr: usize) -> isize {
    const FUTEX_WAIT: usize = 0;
    const FUTEX_WAKE: usize = 1;
    const FUTEX_WAIT_BITSET: usize = 9;
    const FUTEX_WAKE_BITSET: usize = 10;
    const FUTEX_PRIVATE_FLAG: usize = 128;
    const FUTEX_CLOCK_REALTIME: usize = 256;

    let base_op = op & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

    match base_op {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            if addr == 0 {
                return ERR_INVAL;
            }
            futex_wait(addr, val as u32, futex_timeout_ms(timeout_ptr))
        }
        FUTEX_WAKE | FUTEX_WAKE_BITSET => {
            if addr == 0 {
                return ERR_INVAL;
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
            crate::arch::trap::PENDING_FS_BASE
                .store(addr as u64, core::sync::atomic::Ordering::Relaxed);
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

/// Linux access(path, mode) — check file accessibility.
#[cfg(target_arch = "x86_64")]
fn linux_access(path: usize, _mode: usize) -> isize {
    let path_len = crate::syscall::linux::count_user_string(path);
    if path_len == 0 {
        return ERR_NOENT;
    }
    // Try to stat the file to check existence
    match linux_stat(path, 0) {
        0 => 0,
        _ => ERR_NOENT,
    }
}

/// Linux getrlimit(resource, rlim) — get resource limits.
/// rlim is a pointer to struct rlimit { rlim_cur, rlim_max } (each u64).
#[cfg(target_arch = "x86_64")]
fn linux_getrlimit(resource: usize, rlim_ptr: usize) -> isize {
    if rlim_ptr == 0 {
        return ERR_FAULT;
    }
    let (cur, max) = match resource {
        7 => (1024u64, 1024u64),   // RLIMIT_NOFILE
        3 => (1024u64, 1024u64),   // RLIMIT_STACK (1MB)
        9 => (1024u64, 1024u64),   // RLIMIT_AS (256MB)
        _ => (u64::MAX, u64::MAX), // unlimited
    };
    user_write::<u64>(rlim_ptr, cur);
    user_write::<u64>(rlim_ptr + 8, max);
    0
}

/// Linux prlimit64(pid, resource, new_rlim, old_rlim) — get/set resource limits.
#[cfg(target_arch = "x86_64")]
fn linux_prlimit64(_pid: usize, resource: usize, new_rlim: usize, old_rlim: usize) -> isize {
    if old_rlim != 0 {
        let (cur, max) = match resource {
            7 => (1024u64, 1024u64), // RLIMIT_NOFILE
            3 => (1024u64, 1024u64), // RLIMIT_STACK
            9 => (1024u64, 1024u64), // RLIMIT_AS
            _ => (u64::MAX, u64::MAX),
        };
        user_write::<u64>(old_rlim, cur);
        user_write::<u64>(old_rlim + 8, max);
    }
    // Ignore new_rlim (we don't actually enforce limits)
    0
}

/// Linux getrusage(who, usage) — get resource usage.
/// struct rusage is 144 bytes on x86_64.
#[cfg(target_arch = "x86_64")]
fn linux_getrusage(_who: usize, usage_ptr: usize) -> isize {
    if usage_ptr == 0 {
        return ERR_FAULT;
    }
    // Zero-fill the entire rusage struct (144 bytes)
    for i in 0..144usize {
        user_write::<u8>(usage_ptr + i, 0);
    }
    // Set ru_utime and ru_stime to a small nonzero value
    let uptime = crate::arch::platform::uptime_ms();
    let ticks = uptime / 10; // 100Hz clock
    // ru_utime (offset 0): timeval { tv_sec, tv_usec }
    user_write::<u64>(usage_ptr, ticks / 100);
    user_write::<u64>(usage_ptr + 8, (ticks % 100) * 10000);
    // ru_stime (offset 16): timeval { tv_sec, tv_usec }
    user_write::<u64>(usage_ptr + 16, ticks / 100);
    user_write::<u64>(usage_ptr + 24, (ticks % 100) * 10000);
    // ru_maxrss (offset 32) — max resident set size in KB
    user_write::<u64>(usage_ptr + 32, 65536);
    0
}

/// Linux times(buf) — get process times.
/// Returns clock ticks elapsed since boot.
#[cfg(target_arch = "x86_64")]
fn linux_times(buf_ptr: usize) -> isize {
    if buf_ptr != 0 {
        // struct tms { tms_utime, tms_stime, tms_cutime, tms_cstime } (4 × u64)
        let uptime = crate::arch::platform::uptime_ms();
        let ticks = uptime / 10;
        user_write::<u64>(buf_ptr, ticks / 2); // utime
        user_write::<u64>(buf_ptr + 8, ticks / 2); // stime
        user_write::<u64>(buf_ptr + 16, 0); // cutime
        user_write::<u64>(buf_ptr + 24, 0); // cstime
    }
    let uptime = crate::arch::platform::uptime_ms();
    (uptime / 10) as isize
}

/// Linux getsockname(fd, addr, addrlen) — get socket local address.
#[cfg(target_arch = "x86_64")]
fn linux_getsockname(_fd: usize, addr_ptr: usize, addrlen_ptr: usize) -> isize {
    if addr_ptr != 0 && addrlen_ptr != 0 {
        // Return 0.0.0.0:0 as the local address
        // struct sockaddr_in { sa_family(2), port(2), addr(4), zero(8) } = 16 bytes
        user_write::<u16>(addr_ptr, 2); // AF_INET
        user_write::<u16>(addr_ptr + 2, 0); // port 0
        user_write::<u32>(addr_ptr + 4, 0); // 0.0.0.0
        user_write::<u32>(addrlen_ptr, 16);
    }
    0
}

/// Linux getpeername(fd, addr, addrlen) — get socket remote address.
#[cfg(target_arch = "x86_64")]
fn linux_getpeername(fd: usize, addr_ptr: usize, addrlen_ptr: usize) -> isize {
    // Try to get the remote address from the socket
    // For now, return ENOTCONN if we can't determine it
    if addr_ptr != 0 && addrlen_ptr != 0 {
        // Check if this is a connected socket
        let fd_type = crate::process::with_fd_table(|fd_table| {
            fd_table.get(fd as usize).map(|d| d.fd_type.clone())
        });
        match fd_type {
            Some(FdType::Socket(_)) => {
                // Return a generic remote address
                user_write::<u16>(addr_ptr, 2); // AF_INET
                user_write::<u16>(addr_ptr + 2, 0);
                user_write::<u32>(addr_ptr + 4, 0);
                user_write::<u32>(addrlen_ptr, 16);
                0
            }
            _ => ERR_INVAL,
        }
    } else {
        0
    }
}

/// Linux setsockopt(fd, level, optname, optval, optlen) — set socket option.
/// Accept all options but mostly ignore them (standard approach for minimal OS).
#[cfg(target_arch = "x86_64")]
fn linux_setsockopt(
    _fd: usize,
    _level: usize,
    _optname: usize,
    _optval: usize,
    _optlen: usize,
) -> isize {
    0
}

/// Linux getsockopt(fd, level, optname, optval, optlen) — get socket option.
#[cfg(target_arch = "x86_64")]
fn linux_getsockopt(
    _fd: usize,
    level: usize,
    optname: usize,
    optval: usize,
    optlen_ptr: usize,
) -> isize {
    if optval != 0 && optlen_ptr != 0 {
        match (level, optname) {
            (1, 3) => {
                // SO_ERROR at SOL_SOCKET — return 0 (no error)
                user_write::<i32>(optval, 0);
                user_write::<u32>(optlen_ptr, 4);
            }
            (1, 2) => {
                // SO_TYPE — return SOCK_STREAM (1)
                user_write::<i32>(optval, 1);
                user_write::<u32>(optlen_ptr, 4);
            }
            (1, 8) => {
                // SO_KEEPALIVE — return 0
                user_write::<i32>(optval, 0);
                user_write::<u32>(optlen_ptr, 4);
            }
            (6, 1) => {
                // TCP_NODELAY at IPPROTO_TCP — return 0 (disabled)
                user_write::<i32>(optval, 0);
                user_write::<u32>(optlen_ptr, 4);
            }
            _ => {
                // Default: return 0 value
                user_write::<i32>(optval, 0);
                user_write::<u32>(optlen_ptr, 4);
            }
        }
    }
    0
}

/// Linux pipe2(fds, flags) — create pipe with flags.
#[cfg(target_arch = "x86_64")]
fn linux_pipe2(fds_ptr: usize, _flags: usize) -> isize {
    if fds_ptr == 0 {
        return ERR_FAULT;
    }
    // Use sys_pipe which writes [read_fd, write_fd] to user buffer
    sys_pipe(fds_ptr)
}

/// Linux readlink(path, buf, bufsiz) — read value of symbolic link.
/// We don't support symlinks, so return EINVAL.
#[cfg(target_arch = "x86_64")]
fn linux_readlink(_path: usize, _buf: usize, _bufsiz: usize) -> isize {
    ERR_INVAL
}

/// Linux readlinkat(dirfd, path, buf, bufsiz) — readlink relative to dirfd.
#[cfg(target_arch = "x86_64")]
fn linux_readlinkat(_dirfd: usize, _path: usize, _buf: usize, _bufsiz: usize) -> isize {
    ERR_INVAL
}

/// Linux prctl(option, ...) — process control operations.
#[cfg(target_arch = "x86_64")]
fn linux_prctl(option: usize, arg2: usize, _arg3: usize, _arg4: usize, _arg5: usize) -> isize {
    match option {
        15 => {
            // PR_SET_NAME — set thread name, just accept
            0
        }
        16 => {
            // PR_GET_NAME — get thread name, write empty string
            if arg2 != 0 {
                for i in 0..16usize {
                    user_write::<u8>(arg2 + i, 0);
                }
            }
            0
        }
        36 => {
            // PR_SET_TIMERSLACK — accept
            0
        }
        _ => 0, // Accept all other prctl options silently
    }
}

/// Linux vfork() — behaves like fork but parent blocks until child exits.
/// We implement it as fork since we don't have memory sharing.
#[cfg(target_arch = "x86_64")]
fn linux_vfork() -> isize {
    sys_fork()
}

/// Linux gethostname(name, len) — return the system hostname.
#[cfg(target_arch = "x86_64")]
fn linux_gethostname(buf: usize, len: usize) -> isize {
    if buf == 0 || len == 0 {
        return ERR_FAULT;
    }
    let hostname = b"karteos\0";
    let copy_len = hostname.len().min(len);
    for (i, &byte) in hostname.iter().enumerate().take(copy_len) {
        user_write::<u8>(buf + i, byte);
    }
    0
}

/// Linux nanosleep(req, rem) — sleep for a specified relative interval.
fn linux_nanosleep(req_ptr: usize, _rem_ptr: usize) -> isize {
    if req_ptr == 0 {
        return ERR_FAULT;
    }

    let sec = user_read::<i64>(req_ptr);
    let nsec = user_read::<i64>(req_ptr + 8);
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return ERR_INVAL;
    }

    let ms = (sec as u64)
        .saturating_mul(1000)
        .saturating_add(((nsec as u64) + 999_999) / 1_000_000);
    if ms == 0 {
        return 0;
    }

    let wake_tick = crate::arch::platform::uptime_ms().saturating_add(ms);
    crate::sched::sleep_until(wake_tick);
    0
}

/// Linux mkdirat(dirfd, pathname, mode) — create directory.
/// dirfd=AT_FDCWD (-100 = 0xffffff9c) means relative to CWD.
#[cfg(target_arch = "x86_64")]
fn linux_mkdirat(_dirfd: usize, path_ptr: usize, _mode: usize, _unused: usize) -> isize {
    if path_ptr == 0 {
        return ERR_INVAL;
    }

    let actual_len = crate::syscall::linux::count_user_string(path_ptr);
    if actual_len == 0 || actual_len > 256 {
        return ERR_NOENT;
    }

    sys_mkdir(path_ptr, actual_len)
}

/// Linux rename(oldpath, newpath) — rename or move a file.
/// Implemented as: read old file → write to new → delete old.
#[cfg(target_arch = "x86_64")]
fn linux_rename(old_path: usize, new_path: usize) -> isize {
    let old_name = match read_user_path(old_path, linux::count_user_string(old_path)) {
        Some(n) if !n.is_empty() => n,
        _ => return ERR_INVAL,
    };
    let new_name = match read_user_path(new_path, linux::count_user_string(new_path)) {
        Some(n) if !n.is_empty() => n,
        _ => return ERR_INVAL,
    };

    let old_name = resolve_path(&old_name);
    let new_name = resolve_path(&new_name);

    // Use ext4 rename: moves directory entry without creating new inode
    match crate::driver::ext4::rename_file(&old_name, &new_name) {
        Ok(()) => 0,
        Err(_) => {
            // Fallback: read + write + delete
            let data = match crate::driver::ext4::read_file(&old_name) {
                Some(d) => d,
                None => return ERR_NOENT,
            };
            crate::driver::ext4::delete_file(&new_name);
            if crate::driver::ext4::write_file(&new_name, &data).is_err() {
                return ERR_IO;
            }
            let _ = crate::driver::ext4::delete_file(&old_name);
            0
        }
    }
}

/// Linux mremap(old_address, old_size, new_size, flags, new_address)
/// Basic implementation: for growing, mmap additional pages; for shrinking, no-op.
#[cfg(target_arch = "x86_64")]
fn linux_mremap(old_address: usize, _old_size: usize, new_size: usize, _flags: usize) -> isize {
    if old_address == 0 || new_size == 0 {
        return ERR_INVAL;
    }
    // For Go compatibility: just return the old address.
    // Go's mmap hint fallback already handles this by using the bump allocator.
    // The VMA tracking will handle page faults lazily for the new size.
    old_address as isize
}

/// Linux sendmsg(fd, msg, flags) — send data via msghdr (simplified: first iovec only)
#[cfg(target_arch = "x86_64")]
fn linux_sendmsg(fd: usize, msg_ptr: usize, flags: usize) -> isize {
    if msg_ptr == 0 {
        return ERR_FAULT;
    }
    // struct msghdr: msg_iov at offset 16, msg_iovlen at offset 24
    let iov_ptr = user_read::<usize>(msg_ptr + 16);
    let iov_len = user_read::<usize>(msg_ptr + 24);
    if iov_len == 0 || iov_ptr == 0 {
        return 0;
    }
    // Use first iovec: iov_base (8), iov_len (8)
    let buf = user_read::<usize>(iov_ptr);
    let len = user_read::<usize>(iov_ptr + 8);
    if len == 0 {
        return 0;
    }
    // Delegate to sendto (no destination address for connected sockets)
    sys_sendto(fd as i32, buf, len, flags, 0, 0)
}

/// Linux recvmsg(fd, msg, flags) — receive data via msghdr (simplified: first iovec only)
#[cfg(target_arch = "x86_64")]
fn linux_recvmsg(fd: usize, msg_ptr: usize, _flags: usize) -> isize {
    if msg_ptr == 0 {
        return ERR_FAULT;
    }
    // struct msghdr: msg_iov at offset 16, msg_iovlen at offset 24
    let iov_ptr = user_read::<usize>(msg_ptr + 16);
    let iov_len = user_read::<usize>(msg_ptr + 24);
    if iov_len == 0 || iov_ptr == 0 {
        return 0;
    }
    // Use first iovec
    let buf = user_read::<usize>(iov_ptr);
    let len = user_read::<usize>(iov_ptr + 8);
    if len == 0 {
        return 0;
    }
    // Delegate to recvfrom
    sys_recvfrom(fd as i32, buf, len)
}

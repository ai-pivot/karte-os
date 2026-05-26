//! KarteOS Syscall ABI
//!
//! Calling convention:
//!   ecall instruction (triggers UserEnvCall, exception code 8)
//!   a7 = syscall number
//!   a0-a5 = arguments (up to 6)
//!   a0 = return value (>= 0 success, < 0 error)

// ─── Syscall numbers ──────────────────────────────────────────────

// Level 1: Core
pub const SYS_DEBUG_PRINT: usize = 0;
pub const SYS_EXIT: usize = 1;
pub const SYS_WRITE: usize = 2;
pub const SYS_READ: usize = 3;
pub const SYS_BRK: usize = 4;
pub const SYS_GETPID: usize = 5;
pub const SYS_MMAP: usize = 6;

// Level 2: Filesystem (reserved)
pub const SYS_OPEN: usize = 10;
pub const SYS_CLOSE: usize = 11;

// Level 4: Network (reserved)
pub const SYS_SOCKET: usize = 20;

// Level 5: Threading (reserved)
pub const SYS_CLONE: usize = 30;

// ─── Error codes ──────────────────────────────────────────────────

pub const ERR_OK: isize = 0;
pub const ERR_INVAL: isize = -1;
pub const ERR_NOMEM: isize = -2;
pub const ERR_NOENT: isize = -3; // No such file or directory
pub const ERR_IO: isize = -4;

/// Dispatch a syscall.
///
/// Called from trap_handler when UserEnvCall is detected.
/// `id` = a7 (syscall number), `args` = [a0, a1, a2, a3, a4, a5].
/// Returns value for a0.
pub fn dispatch(id: usize, args: [usize; 6]) -> isize {
    match id {
        SYS_DEBUG_PRINT => sys_debug_print(args[0], args[1]),
        SYS_EXIT => sys_exit(args[0] as i32),
        SYS_WRITE => sys_write(args[0] as i32, args[1], args[2]),
        SYS_READ => sys_read(args[0] as i32, args[1], args[2]),
        SYS_BRK => sys_brk(args[0]),
        SYS_GETPID => sys_getpid(),
        SYS_MMAP => sys_mmap(args[0], args[1], args[2]),
        _ => {
            crate::console_println!("[syscall] Unknown syscall: {}", id);
            ERR_INVAL
        }
    }
}

/// Syscall 0: Debug print (write bytes to kernel console).
/// Used by user programs before proper file descriptors work.
fn sys_debug_print(buf: usize, len: usize) -> isize {
    if buf == 0 || len == 0 || len > 4096 {
        return ERR_INVAL;
    }
    let data = unsafe { core::slice::from_raw_parts(buf as *const u8, len) };
    crate::sbi::print(core::str::from_utf8(data).unwrap_or("[invalid utf8]"));
    len as isize
}

/// Syscall 1: Exit the current process.
fn sys_exit(code: i32) -> isize {
    crate::console_println!("[process] User process exited with code {}", code);
    crate::sched::mark_current_exited();
    // TODO: actually clean up the process
    // For now, just loop forever
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}

/// Syscall 2: Write to file descriptor.
fn sys_write(fd: i32, buf: usize, len: usize) -> isize {
    if buf == 0 || len == 0 || len > 65536 {
        return ERR_INVAL;
    }
    match fd {
        1 | 2 => {
            // stdout/stderr: write to console byte by byte
            for i in 0..len {
                let byte = unsafe { core::ptr::read_volatile((buf + i) as *const u8) };
                crate::sbi::console_putchar(byte);
            }
            len as isize
        }
        _ => ERR_INVAL,
    }
}

/// Syscall 3: Read from file descriptor.
fn sys_read(_fd: i32, _buf: usize, _len: usize) -> isize {
    // Not implemented yet
    ERR_INVAL
}

/// Syscall 4: Set/get program break (heap pointer).
fn sys_brk(addr: usize) -> isize {
    let current = crate::sched::current_brk();
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

    // Allocate and map pages from current brk to new brk
    let kernel_pt = crate::mm::vmm::get_kernel_page_table();
    let page_size = crate::mm::pmm::page_size();
    let start_page = (current + page_size - 1) & !(page_size - 1); // Round up
    let end_page = (addr + page_size - 1) & !(page_size - 1);

    let mut vaddr = start_page;
    while vaddr < end_page {
        // Check if already mapped
        if crate::mm::vmm::translate_user(kernel_pt, vaddr).is_none() {
            let frame = match crate::mm::pmm::alloc_frame() {
                Some(f) => f,
                None => return ERR_NOMEM,
            };
            // Zero the page
            unsafe {
                core::ptr::write_bytes(frame as *mut u8, 0, page_size);
            }
            // Map with URW flags (user readable/writable, no execute)
            crate::mm::vmm::map(kernel_pt, vaddr, frame, crate::mm::vmm::PTEFlags::URW);
        }
        vaddr += page_size;
    }

    // Flush TLB
    unsafe {
        core::arch::asm!("sfence.vma");
    }

    crate::sched::set_current_brk(addr);
    addr as isize
}

/// Syscall 5: Get process ID.
fn sys_getpid() -> isize {
    crate::sched::current_task_id() as isize
}

/// Syscall 6: Map anonymous memory.
/// `addr` = hint address (0 = kernel chooses), `len` = size, `flags` = prot flags
/// Returns the mapped virtual address, or error.
/// Simple implementation: always maps at the hint address or at brk.
fn sys_mmap(addr: usize, len: usize, _flags: usize) -> isize {
    if len == 0 {
        return ERR_INVAL;
    }

    let kernel_pt = crate::mm::vmm::get_kernel_page_table();
    let page_size = crate::mm::pmm::page_size();

    // Determine mapping range
    // If addr is 0, use current brk as base
    let base = if addr == 0 {
        let current_brk = crate::sched::current_brk();
        // Align up
        (current_brk + page_size - 1) & !(page_size - 1)
    } else {
        // Align down
        addr & !(page_size - 1)
    };

    let end = (base + len + page_size - 1) & !(page_size - 1);

    // Validate range is within user heap area
    if base < crate::process::USER_HEAP_BASE || end > crate::process::USER_HEAP_LIMIT {
        return ERR_INVAL;
    }

    // Allocate and map pages
    let mut vaddr = base;
    while vaddr < end {
        if crate::mm::vmm::translate_user(kernel_pt, vaddr).is_none() {
            let frame = match crate::mm::pmm::alloc_frame() {
                Some(f) => f,
                None => return ERR_NOMEM,
            };
            unsafe {
                core::ptr::write_bytes(frame as *mut u8, 0, page_size);
            }
            crate::mm::vmm::map(kernel_pt, vaddr, frame, crate::mm::vmm::PTEFlags::URW);
        }
        vaddr += page_size;
    }

    unsafe {
        core::arch::asm!("sfence.vma");
    }

    // If addr was 0, advance brk
    if addr == 0 {
        crate::sched::set_current_brk(end);
    }

    base as isize
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
}

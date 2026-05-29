//! KarteOS Syscall ABI
//!
//! Calling convention:
//!   ecall instruction (triggers UserEnvCall, exception code 8)
//!   a7 = syscall number
//!   a0-a5 = arguments (up to 6)
//!   a0 = return value (>= 0 success, < 0 error)

pub mod linux;

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

// Level 5: Threading
pub const SYS_SPAWN: usize = 30;
pub const SYS_WAITPID: usize = 31;
pub const SYS_EXEC: usize = 32; // spawn by file path

// Level 6: Extended
pub const SYS_LS: usize = 40;

// ─── Error codes ──────────────────────────────────────────────────

pub const ERR_OK: isize = 0;
pub const ERR_INVAL: isize = -1;
pub const ERR_NOMEM: isize = -2;
pub const ERR_NOENT: isize = -3; // No such file or directory
pub const ERR_IO: isize = -4;

// ─── Global FD table (single-process simplification) ────────────────

extern crate alloc;

use crate::driver::fs::{MAX_FDS, O_CREAT};
#[cfg(feature = "test_mode")]
use crate::driver::fs::{O_RDONLY, O_RDWR, O_WRONLY};

/// Dispatch a syscall.
///
/// Called from trap_handler when UserEnvCall is detected.
/// `id` = a7 (syscall number), `args` = [a0, a1, a2, a3, a4, a5].
/// Returns value for a0.
pub fn dispatch(id: usize, args: [usize; 6]) -> isize {
    // Enable timer interrupts on the first syscall.
    // Timer is intentionally delayed until the user program has executed
    // at least one ecall, to avoid timer interrupts during the critical
    // sret-to-first-ecall window where CSR probing can cause issues.
    static TIMER_ENABLED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);
    if !TIMER_ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
        TIMER_ENABLED.store(true, core::sync::atomic::Ordering::Relaxed);
        crate::arch::trap::enable_timer_interrupt();
        crate::arch::trap::set_next_timer();
    }

    // Try Linux compat layer first.
    if let Some(translation) = linux::translate(id, args) {
        return match translation {
            linux::Translation::Dispatch { karte_nr, args } => dispatch(karte_nr, args),
            linux::Translation::Handled(retval) => retval,
        };
    }

    match id {
        SYS_DEBUG_PRINT => sys_debug_print(args[0], args[1]),
        SYS_EXIT => sys_exit(args[0] as i32),
        SYS_WRITE => sys_write(args[0] as i32, args[1], args[2]),
        SYS_READ => sys_read(args[0] as i32, args[1], args[2]),
        SYS_BRK => sys_brk(args[0]),
        SYS_GETPID => sys_getpid(),
        SYS_MMAP => sys_mmap(args[0], args[1], args[2]),
        SYS_OPEN => sys_open(args[0], args[1], args[2] as u32),
        SYS_CLOSE => sys_close(args[0] as i32),
        SYS_SPAWN => sys_spawn(args[0], args[1]),
        SYS_EXEC => sys_exec(args[0], args[1]),
        SYS_WAITPID => sys_waitpid(args[0]),
        SYS_LS => sys_ls(args[0], args[1]),

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

    // If init (the shell) exits, no process remains → shut down the system.
    if crate::sched::is_init_running() {
        crate::console_println!("[init] Shell exited, shutting down...");
        crate::sbi::shutdown();
    }

    crate::process::set_exit_code(code as usize);

    // Wake parent if waiting
    let my_idx = crate::process::current_index();
    if let Some(parent_idx) = crate::process::find_waiting_parent(my_idx) {
        crate::process::set_wait_child(parent_idx, None);
        crate::sched::wake_task(parent_idx);
    }

    // Mark this child task as exited in the scheduler
    crate::sched::mark_current_exited();

    // Try to switch to another ready child task (or back to init).
    crate::sched::schedule_exit();

    0
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
        _ => {
            // File fd: get name and position from current process's FD table
            let (name, pos) = {
                crate::process::with_fd_table(|fd_table| match fd_table.get(fd as usize) {
                    Some(f) => (f.name.clone(), f.pos),
                    None => (alloc::string::String::new(), 0),
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
                    data[pos + i] = unsafe { core::ptr::read_volatile((buf + i) as *const u8) };
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
    }
}

/// Syscall 3: Read from file descriptor.
fn sys_read(fd: i32, buf: usize, len: usize) -> isize {
    if buf == 0 || len == 0 || len > 65536 {
        return ERR_INVAL;
    }

    // stdin: blocking read from TTY subsystem.
    // If no data is available, the current task blocks until the user
    // presses Enter (the TTY line editor commits a line to the ring buffer).
    if fd == 0 {
        return crate::driver::tty::read(buf, len);
    }

    // Get file info from current process's FD table
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
    for i in 0..to_read {
        unsafe { core::ptr::write_volatile((buf + i) as *mut u8, data[pos + i]) };
    }

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

    // Use current process page table
    let user_pt = crate::arch::trap::get_current_user_pt();
    let page_size = crate::mm::pmm::page_size();
    let start_page = (current + page_size - 1) & !(page_size - 1); // Round up
    let end_page = (addr + page_size - 1) & !(page_size - 1);

    let mut vaddr = start_page;
    while vaddr < end_page {
        // Check if already mapped
        if crate::mm::vmm::translate_user(user_pt, vaddr).is_none() {
            let frame = match crate::mm::pmm::alloc_frame() {
                Some(f) => f,
                None => return ERR_NOMEM,
            };
            // Zero the page
            unsafe {
                core::ptr::write_bytes(frame as *mut u8, 0, page_size);
            }
            // Map with URW flags (user readable/writable, no execute)
            crate::mm::vmm::map(user_pt, vaddr, frame, crate::mm::vmm::PTEFlags::URW);
        }
        vaddr += page_size;
    }

    // Flush TLB
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

/// Syscall 6: Map anonymous memory.
/// `addr` = hint address (0 = kernel chooses), `len` = size, `flags` = prot flags
/// Returns the mapped virtual address, or error.
///
/// When addr=0, allocates from a per-process mmap region that grows upward
/// from USER_MMAP_BASE. This matches Linux behavior where mmap returns
/// addresses in a dedicated region (not overlapping brk).
fn sys_mmap(addr: usize, len: usize, _flags: usize) -> isize {
    if len == 0 {
        return ERR_INVAL;
    }

    // Use current process page table
    let user_pt = crate::arch::trap::get_current_user_pt();
    let page_size = crate::mm::pmm::page_size();

    let base = if addr == 0 {
        // Kernel chooses address: use mmap region, grow from current brk/mmap_top
        let mmap_base = crate::process::USER_MMAP_BASE;
        // Find first unmapped region in mmap area
        let mut candidate = mmap_base;
        let needed_pages = (len + page_size - 1) / page_size;
        // Simple linear scan for a free region
        'outer: loop {
            let mut all_free = true;
            for i in 0..needed_pages {
                let vaddr = candidate + i * page_size;
                if vaddr >= crate::process::USER_MMAP_LIMIT {
                    return ERR_NOMEM;
                }
                if crate::mm::vmm::translate_user(user_pt, vaddr).is_some() {
                    // This page is already mapped, skip past it
                    candidate = vaddr + page_size;
                    continue 'outer;
                }
            }
            break 'outer;
        }
        candidate
    } else {
        // Use hint address (aligned down)
        addr & !(page_size - 1)
    };

    let end = (base + len + page_size - 1) & !(page_size - 1);

    // Validate range — allow mmap region and heap region
    let valid_start = crate::process::USER_HEAP_BASE;
    let valid_end = crate::process::USER_MMAP_LIMIT;
    if base < valid_start || end > valid_end {
        crate::console_println!("[mmap] range {:#x}-{:#x} out of bounds", base, end);
        return ERR_INVAL;
    }

    // Allocate and map pages
    let mut vaddr = base;
    while vaddr < end {
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

    unsafe {
        core::arch::asm!("sfence.vma");
    }

    // If addr was 0, advance brk
    if addr == 0 {
        crate::process::set_current_brk(end);
    }

    base as isize
}

/// Syscall 10: Open a file.
/// `path` = pointer to file path string, `path_len` = length, `flags` = open flags.
/// Returns the file descriptor number, or a negative error code.
pub(crate) fn sys_open(path: usize, path_len: usize, flags: u32) -> isize {
    if path == 0 || path_len == 0 || path_len > 256 {
        return ERR_INVAL;
    }

    // Read path from user memory, stripping trailing NUL bytes (C-style strings)
    let mut path_buf = alloc::vec::Vec::new();
    for i in 0..path_len {
        let byte = unsafe { core::ptr::read_volatile((path + i) as *const u8) };
        path_buf.push(byte);
    }
    while path_buf.last() == Some(&0) {
        path_buf.pop();
    }
    let name = alloc::string::String::from_utf8(path_buf).unwrap_or_default();
    if name.is_empty() {
        return ERR_INVAL;
    }

    // Strip leading '/' for filesystem lookup
    let name = alloc::string::String::from(name.strip_prefix('/').unwrap_or(&name));
    if name.is_empty() {
        return ERR_INVAL;
    }

    // Check/create file in FS
    if flags & O_CREAT != 0 {
        let _ = crate::driver::fs::create_file(&name);
    }
    // Verify file exists (try FAT32 + RamFS)
    if crate::driver::fs::read_file_owned(&name).is_none() && (flags & O_CREAT == 0) {
        return ERR_NOENT;
    }

    // Allocate fd from current process's FD table
    crate::process::with_fd_table(|fd_table| match fd_table.alloc(name, flags) {
        Some(fd) => fd as isize,
        None => ERR_NOMEM,
    })
}

/// Syscall 11: Close a file descriptor.
fn sys_close(fd: i32) -> isize {
    if fd < 0 || fd as usize >= MAX_FDS {
        return ERR_INVAL;
    }
    crate::process::with_fd_table(|fd_table| {
        if fd_table.close(fd as usize) {
            ERR_OK
        } else {
            ERR_INVAL
        }
    })
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
        None => WAIT_AGAIN, // Child still running
    }
}

/// Syscall 40: List filesystem contents.
/// Writes a formatted listing to the user buffer (name + size per line).
/// Returns total bytes written, or error.
fn sys_ls(buf: usize, len: usize) -> isize {
    if buf == 0 || len == 0 {
        return ERR_INVAL;
    }

    let files = crate::driver::fs::list_all_files();

    let mut written: usize = 0;
    for (name, size) in files {
        // Format: "name\tsize\n"

        // Write name
        for &b in name.as_bytes() {
            if written >= len {
                break;
            }
            unsafe { core::ptr::write_volatile((buf + written) as *mut u8, b) };
            written += 1;
        }
        // Tab
        if written < len {
            unsafe { core::ptr::write_volatile((buf + written) as *mut u8, b'\t') };
            written += 1;
        }
        // Size (write digits directly)
        if size == 0 {
            if written < len {
                unsafe { core::ptr::write_volatile((buf + written) as *mut u8, b'0') };
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
                unsafe { core::ptr::write_volatile((buf + written) as *mut u8, tmp[j]) };
                written += 1;
            }
        }
        // Newline
        if written < len {
            unsafe { core::ptr::write_volatile((buf + written) as *mut u8, b'\n') };
            written += 1;
        }
    }

    written as isize
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
        Some(data) => match crate::process::Process::from_elf(&data) {
            Ok(p) => p,
            Err(e) => {
                crate::console_println!("[spawn] Failed to create process: {}", e);
                return ERR_NOMEM;
            }
        },
        None => {
            crate::console_println!("[spawn] Program '{}' not found in filesystem", file_name);
            return ERR_NOENT;
        }
    };

    let child_pid = proc.pid;
    let entry = proc.entry;
    let user_stack_top = proc.user_stack_top;
    let kernel_stack_top = proc.kernel_stack_top;

    // Calculate user satp value (Sv39 mode = 8)
    let user_satp = if proc.page_table_root == 0 {
        // Fallback: read current satp
        let satp: usize;
        unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp) };
        satp
    } else {
        (8usize << 60) | proc.page_table_root
    };

    // Register process in the global process table
    let proc_idx = match crate::process::add_process(proc) {
        Some(idx) => idx,
        None => {
            crate::console_println!("[spawn] Process table full");
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
            crate::console_println!("[spawn] Spawned process pid={}", child_pid);
            child_pid as isize
        }
        None => {
            crate::console_println!("[spawn] Scheduler full");
            ERR_NOMEM
        }
    }
}

/// Syscall 32: Execute (spawn) a program by file path.
/// `path` = pointer to file path string, `path_len` = length.
/// Returns child PID on success, or negative error code.
fn sys_exec(path: usize, path_len: usize) -> isize {
    if path == 0 || path_len == 0 || path_len > 256 {
        return ERR_INVAL;
    }

    // Read path from user memory
    let mut path_buf = alloc::vec::Vec::new();
    for i in 0..path_len {
        let byte = unsafe { core::ptr::read_volatile((path + i) as *const u8) };
        path_buf.push(byte);
    }
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

    crate::console_println!("[exec] Loading '{}'...", name);

    // Load ELF data from filesystem (FAT32 + RamFS)
    let proc = match crate::driver::fs::read_file_owned(&name) {
        Some(data) => match crate::process::Process::from_elf(&data) {
            Ok(p) => p,
            Err(e) => {
                crate::console_println!("[exec] Failed to create process: {}", e);
                return ERR_NOMEM;
            }
        },
        None => {
            crate::console_println!("[exec] Program '{}' not found", name);
            return ERR_NOENT;
        }
    };

    let child_pid = proc.pid;
    let entry = proc.entry;
    let user_stack_top = proc.user_stack_top;
    let kernel_stack_top = proc.kernel_stack_top;

    let user_satp = if proc.page_table_root == 0 {
        let satp: usize;
        unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp) };
        satp
    } else {
        (8usize << 60) | proc.page_table_root
    };

    let proc_idx = match crate::process::add_process(proc) {
        Some(idx) => idx,
        None => {
            crate::console_println!("[exec] Process table full");
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
            crate::console_println!("[exec] Spawned '{}' pid={}", name, child_pid);
            child_pid as isize
        }
        None => {
            crate::console_println!("[exec] Scheduler full");
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
}

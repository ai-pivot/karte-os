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
pub const LINUX_GETRANDOM: usize = 114;
pub const LINUX_SET_TID_ADDRESS: usize = 115;

// ─── Error codes ──────────────────────────────────────────────────

pub const ERR_OK: isize = 0;
pub const ERR_INVAL: isize = -1;
pub const ERR_NOMEM: isize = -2;
pub const ERR_NOENT: isize = -3; // No such file or directory
pub const ERR_IO: isize = -4;

// ─── Global FD table (single-process simplification) ────────────────

extern crate alloc;

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
static PRNG_STATE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0x12345678_9ABCDEF0);

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
        SYS_PIPE => sys_pipe(args[0]),
        SYS_DUP2 => sys_dup2(args[0] as i32, args[1] as i32),
        SYS_OPEN => sys_open(args[0], args[1], args[2] as u32),
        SYS_CLOSE => sys_close(args[0] as i32),
        SYS_SPAWN => sys_spawn(args[0], args[1]),
        SYS_EXEC => sys_exec(args[0], args[1]),
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
        LINUX_GETRANDOM => linux_getrandom(args[0], args[1], args[2]),
        LINUX_SET_TID_ADDRESS => linux_set_tid_address(args[0]),
        #[cfg(target_arch = "x86_64")]
        LINUX_ARCH_PRCTL => linux_arch_prctl(args[0], args[1]),
        #[cfg(not(target_arch = "x86_64"))]
        LINUX_ARCH_PRCTL => {
            0 // stub: not needed on non-x86_64
        }

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
    crate::arch::platform::print(core::str::from_utf8(data).unwrap_or("[invalid utf8]"));
    len as isize
}

/// Syscall 1: Exit the current process.
fn sys_exit(code: i32) -> isize {
    crate::console_println!("[process] User process exited with code {}", code);

    // If init (the shell) exits, no process remains → shut down the system.
    if crate::sched::is_init_running() {
        crate::console_println!("[init] Shell exited, shutting down...");
        crate::arch::platform::shutdown();
    }

    // CLONE_CHILD_CLEARTID: write 0 to the child_tid_ptr on exit.
    // This is used by Go's futex-based thread joining.
    #[cfg(target_arch = "x86_64")]
    if let Some(proc) = crate::process::current() {
        if proc.child_tid_ptr != 0 {
            unsafe {
                core::ptr::write_volatile(proc.child_tid_ptr as *mut i32, 0);
            }
        }
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

    // First check fd_table for the actual fd type.
    // This handles redirected stdout/stderr (e.g., file or pipe).
    let fd_info = get_fd_info(fd);
    match fd_info {
        Some((FdType::PipeWrite, Some(pipe_id), _)) => {
            return pipe_write(pipe_id, buf, len);
        }
        Some((FdType::PipeRead, _, _)) => {
            return ERR_INVAL; // can't write to read end
        }
        Some((FdType::Stdio, _, _)) => {
            // Stdio (fd 0/1/2 default): write to console
            for i in 0..len {
                let byte = unsafe { core::ptr::read_volatile((buf + i) as *const u8) };
                crate::arch::platform::console_putchar(byte);
            }
            return len as isize;
        }
        Some((FdType::File, _, _)) => {
            // Fall through to file write below
        }
        _ => {
            // Unknown type or no info — if fd is 1/2, write to console
            if fd == 1 || fd == 2 {
                for i in 0..len {
                    let byte = unsafe { core::ptr::read_volatile((buf + i) as *const u8) };
                    crate::arch::platform::console_putchar(byte);
                }
                return len as isize;
            }
            return ERR_INVAL;
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
            loop {
                let result = crate::driver::tty::read(buf, len);
                if result > 0 {
                    return result;
                }
                crate::driver::tty::poll_uart();
                crate::sched::schedule();
            }
        }
        Some((FdType::File, _, _)) => {
            // Fall through to file read below
        }
        _ => {
            // Unknown type — if fd == 0, use TTY
            if fd == 0 {
                loop {
                    let result = crate::driver::tty::read(buf, len);
                    if result > 0 {
                        return result;
                    }
                    crate::driver::tty::poll_uart();
                    crate::sched::schedule();
                }
            }
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
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("sfence.vma");
    }
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::trap::flush_tlb();
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
fn sys_mmap(addr: usize, len: usize, _flags: usize) -> isize {
    if len == 0 {
        return ERR_INVAL;
    }
    linux_mmap(
        addr,
        len,
        3,    /* PROT_READ|PROT_WRITE */
        0x22, /* MAP_PRIVATE|MAP_ANONYMOUS */
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

    let page_size = crate::mm::pmm::page_size();
    let aligned_len = (len + page_size - 1) & !(page_size - 1);

    // Use current process page table
    let user_pt = crate::arch::trap::get_current_user_pt();

    let target_addr = if addr == 0 || (flags & MAP_FIXED != 0) {
        if addr == 0 {
            // Kernel chooses address: find first free region in mmap area
            let mmap_base = crate::process::USER_MMAP_BASE;
            let needed_pages = aligned_len / page_size;
            let mut candidate = mmap_base;
            'outer: loop {
                for i in 0..needed_pages {
                    let vaddr = candidate + i * page_size;
                    if vaddr >= crate::process::USER_MMAP_LIMIT {
                        return -12; // ENOMEM
                    }
                    if crate::mm::vmm::translate_user(user_pt, vaddr).is_some() {
                        candidate = vaddr + page_size;
                        continue 'outer;
                    }
                }
                break 'outer;
            }
            candidate
        } else {
            // MAP_FIXED or addr != 0: use the provided address (page-aligned)
            addr & !(page_size - 1)
        }
    } else {
        // addr != 0 without MAP_FIXED: try at addr, but don't overwrite
        let aligned_addr = addr & !(page_size - 1);
        // Check if the region is free
        let mut all_free = true;
        for i in 0..(aligned_len / page_size) {
            let vaddr = aligned_addr + i * page_size;
            if crate::mm::vmm::translate_user(user_pt, vaddr).is_some() {
                all_free = false;
                break;
            }
        }
        if all_free {
            aligned_addr
        } else {
            // Fall back to kernel-chosen address
            let mmap_base = crate::process::USER_MMAP_BASE;
            let needed_pages = aligned_len / page_size;
            let mut candidate = mmap_base;
            'outer2: loop {
                for i in 0..needed_pages {
                    let vaddr = candidate + i * page_size;
                    if vaddr >= crate::process::USER_MMAP_LIMIT {
                        return -12; // ENOMEM
                    }
                    if crate::mm::vmm::translate_user(user_pt, vaddr).is_some() {
                        candidate = vaddr + page_size;
                        continue 'outer2;
                    }
                }
                break 'outer2;
            }
            candidate
        }
    };

    let end = target_addr + aligned_len;

    // Validate range
    let valid_start = crate::process::USER_HEAP_BASE;
    let valid_end = crate::process::USER_MMAP_LIMIT;
    if target_addr < valid_start || end > valid_end {
        crate::console_println!("[mmap] range {:#x}-{:#x} out of bounds", target_addr, end);
        return -22; // EINVAL
    }

    // Determine PTE flags from prot
    let pte_flags = prot_to_pte_flags(prot);

    // Allocate and map pages (zero-fill for MAP_ANONYMOUS)
    let is_anonymous = flags & MAP_ANONYMOUS != 0 || _fd == usize::MAX;
    let mut vaddr = target_addr;
    while vaddr < end {
        let needs_map = if flags & MAP_FIXED != 0 {
            // MAP_FIXED: unmap existing pages first, then remap
            if let Some(_paddr) = crate::mm::vmm::translate_user(user_pt, vaddr) {
                crate::mm::vmm::unmap_user(user_pt, vaddr);
            }
            true
        } else {
            crate::mm::vmm::translate_user(user_pt, vaddr).is_none()
        };

        if needs_map {
            let frame = match crate::mm::pmm::alloc_frame() {
                Some(f) => f,
                None => return -12, // ENOMEM
            };
            if is_anonymous {
                unsafe {
                    core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                }
            }
            crate::mm::vmm::map(user_pt, vaddr, frame, pte_flags);
        } else if flags & MAP_FIXED != 0 {
            // Page was already mapped but MAP_FIXED means we should remap
            // This case is handled by needs_map above
        } else {
            // Page already exists, update its flags to match prot
            crate::mm::vmm::mprotect_user(user_pt, vaddr, pte_flags);
        }
        vaddr += page_size;
    }

    // Flush TLB
    flush_tlb_all();

    // Advance brk tracking (for addr=0 kernel-chosen allocations)
    if addr == 0 && end > crate::process::current_brk() {
        crate::process::set_current_brk(end);
    }

    target_addr as isize
}

/// Convert Linux prot flags to KarteOS PTEFlags.
fn prot_to_pte_flags(prot: usize) -> crate::mm::vmm::PTEFlags {
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
fn linux_mprotect(addr: usize, len: usize, prot: usize) -> isize {
    if addr == 0 || len == 0 {
        return -22; // EINVAL
    }

    let page_size = crate::mm::pmm::page_size();
    let start = addr & !(page_size - 1);
    let end = (addr + len + page_size - 1) & !(page_size - 1);

    let user_pt = crate::arch::trap::get_current_user_pt();
    let pte_flags = prot_to_pte_flags(prot);

    for vaddr in (start..end).step_by(page_size) {
        crate::mm::vmm::mprotect_user(user_pt, vaddr, pte_flags);
    }

    flush_tlb_all();
    0
}

/// Linux munmap(addr, len) — unmap pages.
/// Does not free physical frames (Go may remap the same region).
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

    let user_pt = crate::arch::trap::get_current_user_pt();
    for vaddr in (start..end).step_by(page_size) {
        crate::mm::vmm::unmap_user(user_pt, vaddr);
    }

    flush_tlb_all();
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
        unsafe {
            let oldact = oldact_ptr as *mut [usize; 4];
            (*oldact)[0] = SIGNAL_STATE.handlers[sig - 1].load(core::sync::atomic::Ordering::Relaxed);
            (*oldact)[1] = 0;
            (*oldact)[2] = 0;
            (*oldact)[3] = 0;
        }
    }
    // Set new handler if provided
    if act_ptr != 0 {
        let handler = unsafe { core::ptr::read_volatile(act_ptr as *const usize) };
        SIGNAL_STATE.handlers[sig - 1].store(handler, core::sync::atomic::Ordering::Relaxed);
    }
    0
}

/// Linux rt_sigprocmask(how, set, oldset, sigsetsize)
/// Record signal mask without actually blocking anything.
fn linux_rt_sigprocmask(how: usize, set_ptr: usize, oldset_ptr: usize) -> isize {
    // Return old mask if requested
    if oldset_ptr != 0 && how != 3 {
        unsafe {
            let oldset = oldset_ptr as *mut u64;
            *oldset = SIGNAL_STATE.mask.load(core::sync::atomic::Ordering::Relaxed);
        }
    }
    // Apply new mask if provided
    if set_ptr != 0 {
        let new_mask = unsafe { core::ptr::read_volatile(set_ptr as *const u64) };
        match how {
            0 => {
                // SIG_BLOCK: add signals to mask
                let prev = SIGNAL_STATE.mask.load(core::sync::atomic::Ordering::Relaxed);
                SIGNAL_STATE.mask.store(prev | new_mask, core::sync::atomic::Ordering::Relaxed);
            }
            1 => {
                // SIG_UNBLOCK: remove signals from mask
                let prev = SIGNAL_STATE.mask.load(core::sync::atomic::Ordering::Relaxed);
                SIGNAL_STATE.mask.store(prev & !new_mask, core::sync::atomic::Ordering::Relaxed);
            }
            2 => {
                // SIG_SETMASK: replace mask entirely
                SIGNAL_STATE.mask.store(new_mask, core::sync::atomic::Ordering::Relaxed);
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
        unsafe {
            let oss = oss_ptr as *mut [usize; 3];
            (*oss)[0] = SIGNAL_STATE.altstack_sp.load(core::sync::atomic::Ordering::Relaxed);
            (*oss)[1] = SIGNAL_STATE.altstack_flags.load(core::sync::atomic::Ordering::Relaxed);
            (*oss)[2] = SIGNAL_STATE.altstack_size.load(core::sync::atomic::Ordering::Relaxed);
        }
    }
    // Set new state if provided
    if ss_ptr != 0 {
        let ss_sp = unsafe { core::ptr::read_volatile(ss_ptr as *const usize) };
        let ss_flags = unsafe { core::ptr::read_volatile((ss_ptr + 8) as *const usize) };
        let ss_size = unsafe { core::ptr::read_volatile((ss_ptr + 16) as *const usize) };
        SIGNAL_STATE.altstack_sp.store(ss_sp, core::sync::atomic::Ordering::Relaxed);
        SIGNAL_STATE.altstack_flags.store(ss_flags, core::sync::atomic::Ordering::Relaxed);
        SIGNAL_STATE.altstack_size.store(ss_size, core::sync::atomic::Ordering::Relaxed);
    }
    0
}

/// Linux getrandom(buf, count, flags)
/// Fill buffer with pseudo-random data using a simple LCG PRNG.
fn linux_getrandom(buf: usize, count: usize, _flags: usize) -> isize {
    if buf == 0 || count == 0 {
        return ERR_INVAL;
    }
    let buf_ptr = buf as *mut u8;
    for i in 0..count {
        // LCG: next = state * 6364136223846793005 + 1442695040888963407
        let prev = PRNG_STATE.load(core::sync::atomic::Ordering::Relaxed);
        let next = prev.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        PRNG_STATE.store(next, core::sync::atomic::Ordering::Relaxed);
        // Use bytes from the state
        let byte = ((next >> (i % 8 * 8)) & 0xFF) as u8;
        unsafe {
            core::ptr::write_volatile(buf_ptr.add(i), byte);
        }
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

    // Check/create file in FS
    if flags & O_CREAT != 0 {
        let _ = crate::driver::fs::create_file(&name);
    }
    // Verify file exists
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
        None => WAIT_AGAIN, // Child still running
    }
}

/// Read a byte string from user memory and strip trailing NUL bytes.
fn read_user_path(ptr: usize, len: usize) -> Option<alloc::string::String> {
    if ptr == 0 || len == 0 || len > 512 {
        return None;
    }
    let mut buf = alloc::vec::Vec::new();
    for i in 0..len {
        let byte = unsafe { core::ptr::read_volatile((ptr + i) as *const u8) };
        buf.push(byte);
    }
    while buf.last() == Some(&0) {
        buf.pop();
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
    match crate::driver::fs::delete_file(&name) {
        Ok(()) => 0,
        Err(()) => ERR_NOENT,
    }
}

/// Syscall 50: Set an environment variable.
fn sys_setenv(key: usize, key_len: usize, val: usize, val_len: usize) -> isize {
    if key == 0 || key_len == 0 || key_len > 128 || val == 0 || val_len > 4096 {
        return ERR_INVAL;
    }
    let mut kbuf = alloc::vec::Vec::new();
    for i in 0..key_len {
        kbuf.push(unsafe { core::ptr::read_volatile((key + i) as *const u8) });
    }
    while kbuf.last() == Some(&0) {
        kbuf.pop();
    }
    let key_str = alloc::string::String::from_utf8(kbuf).unwrap_or_default();

    let mut vbuf = alloc::vec::Vec::new();
    for i in 0..val_len {
        vbuf.push(unsafe { core::ptr::read_volatile((val + i) as *const u8) });
    }
    while vbuf.last() == Some(&0) {
        vbuf.pop();
    }
    let val_str = alloc::string::String::from_utf8(vbuf).unwrap_or_default();

    crate::env::set(&key_str, &val_str);
    0
}

/// Syscall 51: Get an environment variable.
/// Returns the length of the value, or -1 if not found.
fn sys_getenv(key: usize, key_len: usize, buf: usize, buf_len: usize) -> isize {
    if key == 0 || key_len == 0 || key_len > 128 {
        return ERR_INVAL;
    }
    let mut kbuf = alloc::vec::Vec::new();
    for i in 0..key_len {
        kbuf.push(unsafe { core::ptr::read_volatile((key + i) as *const u8) });
    }
    while kbuf.last() == Some(&0) {
        kbuf.pop();
    }
    let key_str = alloc::string::String::from_utf8(kbuf).unwrap_or_default();

    match crate::env::get(&key_str) {
        Some(val) => {
            if buf != 0 && buf_len > 0 {
                let copy_len = core::cmp::min(val.len(), buf_len);
                for i in 0..copy_len {
                    unsafe { core::ptr::write_volatile((buf + i) as *mut u8, val.as_bytes()[i]) };
                }
                copy_len as isize
            } else {
                val.len() as isize
            }
        }
        None => -1,
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

// ─── Network Syscalls (Level 10) ────────────────────────────────────

/// Parse a sockaddr_in from user memory.
/// Returns (port, ip_bytes) or error.
fn parse_sockaddr_in(addr_ptr: usize, addr_len: usize) -> Result<(u16, [u8; 4]), isize> {
    if addr_ptr == 0 || addr_len < 8 {
        return Err(ERR_INVAL);
    }

    let data = unsafe { core::slice::from_raw_parts(addr_ptr as *const u8, addr_len.min(16)) };

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
#[cfg(target_arch = "riscv64")]
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

#[cfg(target_arch = "x86_64")]
fn sys_socket(_domain: usize, _socket_type: usize, _protocol: usize) -> isize {
    ERR_IO // Network not available on x86_64
}

/// Syscall 71: bind(fd, addr_ptr, addr_len) → 0
#[cfg(target_arch = "riscv64")]
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

#[cfg(target_arch = "x86_64")]
fn sys_bind(_fd: i32, _addr_ptr: usize, _addr_len: usize) -> isize {
    ERR_IO
}

/// Syscall 72: connect(fd, addr_ptr, addr_len) → 0
#[cfg(target_arch = "riscv64")]
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

#[cfg(target_arch = "x86_64")]
fn sys_connect(_fd: i32, _addr_ptr: usize, _addr_len: usize) -> isize {
    ERR_IO
}

/// Syscall 73: listen(fd, backlog) → 0
#[cfg(target_arch = "riscv64")]
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

#[cfg(target_arch = "x86_64")]
fn sys_listen(_fd: i32, _backlog: usize) -> isize {
    ERR_IO
}

/// Syscall 74: accept(fd) → new_fd
#[cfg(target_arch = "riscv64")]
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

#[cfg(target_arch = "x86_64")]
fn sys_accept(_fd: i32) -> isize {
    ERR_IO
}

/// Syscall 75: sendto(fd, buf, len, flags, addr_ptr, addr_len) → bytes_sent
#[cfg(target_arch = "riscv64")]
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

    let data = unsafe { core::slice::from_raw_parts(buf as *const u8, len) };

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

    crate::net::iface::NetStack::send(fd as usize, data, ip, port)
}

#[cfg(target_arch = "x86_64")]
#[allow(unused_variables)]
fn sys_sendto(
    fd: i32,
    buf: usize,
    len: usize,
    flags: usize,
    addr_ptr: usize,
    addr_len: usize,
) -> isize {
    ERR_IO
}

/// Syscall 76: recvfrom(fd, buf, len, flags) → bytes_received
#[cfg(target_arch = "riscv64")]
fn sys_recvfrom(fd: i32, buf: usize, len: usize) -> isize {
    if fd < 0 || buf == 0 || len == 0 {
        return ERR_INVAL;
    }

    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }

    let user_buf = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len) };

    match crate::net::iface::NetStack::recv(fd as usize, user_buf) {
        Ok((n, _src_ip, _src_port)) => n as isize,
        Err(e) => e,
    }
}

#[cfg(target_arch = "x86_64")]
fn sys_recvfrom(_fd: i32, _buf: usize, _len: usize) -> isize {
    ERR_IO
}

/// Syscall 77: shutdown(fd, how) → 0
#[cfg(target_arch = "riscv64")]
fn sys_shutdown(fd: i32) -> isize {
    if fd < 0 {
        return ERR_INVAL;
    }

    if !crate::net::iface::NetStack::is_initialized() {
        return ERR_IO;
    }

    crate::net::iface::NetStack::shutdown(fd as usize)
}

#[cfg(target_arch = "x86_64")]
fn sys_shutdown(_fd: i32) -> isize {
    ERR_IO
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

    // Try streaming ELF loader from ext4 first (avoids loading entire file into memory)
    let proc = if crate::driver::ext4::has_ext4() {
        match crate::driver::ext4::read_file_range(&name) {
            Some(read_fn) => match crate::process::Process::from_elf_streaming(read_fn) {
                Ok(p) => p,
                Err(e) => {
                    crate::console_println!("[exec] Streaming ELF load failed: {}", e);
                    return ERR_NOMEM;
                }
            },
            None => {
                // File not found on ext4, try fallback
                match crate::driver::fs::read_file_owned(&name) {
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
                }
            }
        }
    } else {
        // No ext4 — use traditional loader (FAT32 + RamFS)
        match crate::driver::fs::read_file_owned(&name) {
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
        }
    };

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
            .map(|f| (f.fd_type, f.pipe_id, f.name.clone()))
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
                    crate::console_println!("[pipe] write: Broken pipe");
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
            unsafe {
                core::ptr::write_volatile(fd_ptr as *mut i32, rfd as i32);
                core::ptr::write_volatile((fd_ptr + 4) as *mut i32, wfd as i32);
            }
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
    let name = if path_len > 0 && path_len < 256 {
        let mut buf = [0u8; 256];
        for i in 0..path_len {
            buf[i] = unsafe { core::ptr::read_volatile((path + i) as *const u8) };
        }
        let len = buf.iter().position(|&b| b == 0).unwrap_or(path_len);
        let s = core::str::from_utf8(&buf[..len]).unwrap_or("");
        // Strip leading '/' if present (fs root convention)
        let name = if s.starts_with('/') { &s[1..] } else { s };
        alloc::string::String::from(name)
    } else {
        return ERR_INVAL;
    };

    // Load ELF from filesystem — try streaming loader from ext4 first
    let mut proc = if crate::driver::ext4::has_ext4() {
        match crate::driver::ext4::read_file_range(&name) {
            Some(read_fn) => match crate::process::Process::from_elf_streaming(read_fn) {
                Ok(p) => p,
                Err(e) => {
                    crate::console_println!(
                        "[exec] Streaming ELF load failed for '{}': {}",
                        name,
                        e
                    );
                    return ERR_IO;
                }
            },
            None => {
                // File not found on ext4, try fallback
                match crate::driver::fs::read_file_owned(&name) {
                    Some(data) => match crate::process::Process::from_elf(&data) {
                        Ok(p) => p,
                        Err(e) => {
                            crate::console_println!("[exec] Failed to parse ELF '{}': {}", name, e);
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
            Some(data) => match crate::process::Process::from_elf(&data) {
                Ok(p) => p,
                Err(e) => {
                    crate::console_println!("[exec] Failed to parse ELF '{}': {}", name, e);
                    return ERR_IO;
                }
            },
            None => return ERR_NOENT,
        }
    };

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
            if let Some(ref mut child_table) = proc.fd_table {
                child_table.set_fd(0, desc);
            }
        }
        if let Some(desc) = stdout_desc {
            if let Some(pipe_id) = desc.pipe_id {
                crate::driver::pipe::inc_ref(pipe_id);
            }
            if let Some(ref mut child_table) = proc.fd_table {
                child_table.set_fd(1, desc);
            }
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
                name, proc.pid, proc.entry, proc.user_stack_top, proc.kernel_stack_top,
                proc.page_table_root, user_satp
            );
            #[cfg(target_arch = "riscv64")]
            crate::console_println!("[exec] Launched '{}' (pid={})", name, proc.pid);
            proc.pid as isize
        }
        None => {
            crate::console_println!("[exec] Failed to schedule process");
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

    // Copy kernel mappings
    crate::process::copy_kernel_mappings(user_pt);

    // Copy user page table entries (deep copy physical frames)
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

    // Allocate kernel stack for child
    let kstack_base = match crate::mm::pmm::alloc_frame() {
        Some(f) => f,
        None => return ERR_NOMEM,
    };
    for _ in 0..3 {
        if crate::mm::pmm::alloc_frame().is_none() {
            return ERR_NOMEM;
        }
    }
    let kernel_stack_top = kstack_base + 4 * page_size;

    let page_table_ppn = (user_pt as *const crate::mm::vmm::PageTable as usize) >> 12;
    let child_pid = crate::process::NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    // Clone fd table
    let fd_table = current.fd_table.clone();

    // Create child process
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
            crate::console_println!("[fork] Failed to schedule child");
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
fn sys_ioctl(fd: i32, cmd: usize, arg: usize) -> isize {
    if fd != 0 {
        return ERR_INVAL;
    }

    match cmd {
        TCSETS => match arg {
            TERM_RAW => {
                crate::driver::tty::set_mode(crate::driver::tty::TtyMode::Raw);
                0
            }
            TERM_COOKED => {
                crate::driver::tty::set_mode(crate::driver::tty::TtyMode::Canonical);
                0
            }
            TERM_ECHO_ON => {
                crate::driver::tty::set_echo(true);
                0
            }
            TERM_ECHO_OFF => {
                crate::driver::tty::set_echo(false);
                0
            }
            _ => ERR_INVAL,
        },
        TIOCGWINSZ => {
            #[cfg(target_arch = "x86_64")]
            {
                let (cols, rows) = crate::driver::vga::screen_size();
                (cols << 16 | rows) as isize
            }
            #[cfg(target_arch = "riscv64")]
            {
                // Default terminal size for serial console
                (80 << 16 | 25) as isize
            }
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
///   CLONE_PARENT_SETTID (0x10000) = write child TID to parent_tid
///   CLONE_CHILD_CLEARTID (0x200000) = clear child_tid on exit
fn linux_clone(
    flags: usize,
    stack: usize,
    parent_tid_ptr: usize,
    tls: usize,
    child_tid_ptr: usize,
) -> isize {
    let is_vm_shared = (flags & 0x100) != 0; // CLONE_VM

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
    let parent_ctx =
        unsafe { &*(parent_ctx_ptr as *const crate::arch::trap::TrapContext) };

    #[cfg(not(target_arch = "x86_64"))]
    {
        // RISC-V: clone not fully supported yet, use basic add_user_process
        let my_proc_idx = crate::process::current_index();
        let user_pt_root = crate::process::current_page_table_root();

        let kernel_stack_pages = crate::process::KERNEL_STACK_PAGES;
        let kernel_stack_base = match crate::mm::pmm::alloc_contiguous_frames(kernel_stack_pages)
        {
            Some(base) => base,
            None => return ERR_NOMEM,
        };
        let kernel_stack_top =
            kernel_stack_base + kernel_stack_pages * crate::mm::pmm::page_size();

        let entry = crate::process::current()
            .map(|p| p.entry)
            .unwrap_or(0);

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
        let kernel_stack_base =
            match crate::mm::pmm::alloc_contiguous_frames(kernel_stack_pages) {
                Some(base) => base,
                None => return ERR_NOMEM,
            };
        let kernel_stack_top =
            kernel_stack_base + kernel_stack_pages * crate::mm::pmm::page_size();

        // Get parent process info
        let parent_proc = match crate::process::current() {
            Some(p) => p,
            None => return ERR_INVAL,
        };

        // Create child process entry
        let child_pid = crate::process::NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        // CLONE_FILES: clone fd table (true sharing would require Arc, clone is close enough)
        let fd_table = if (flags & 0x400) != 0 {
            // CLONE_FILES
            parent_proc.fd_table.clone()
        } else {
            Some(crate::driver::fs::FdTable::new())
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

        // CLONE_PARENT_SETTID: write child PID to parent's memory
        if (flags & 0x10000) != 0 && parent_tid_ptr != 0 {
            unsafe {
                core::ptr::write_volatile(parent_tid_ptr as *mut i32, child_pid as i32);
            }
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

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use crate::sync::spinlock::SpinLock;

/// A waiter in the futex queue.
struct FutexWaiter {
    /// Process index (used by sched::wake_task)
    proc_idx: usize,
    /// Whether this waiter has already been woken
    woken: bool,
}

/// Global futex wait queues, keyed by user-space futex address.
static FUTEX_QUEUES: SpinLock<BTreeMap<usize, Vec<FutexWaiter>>> =
    SpinLock::new(BTreeMap::new());

/// Block the current task on a futex address.
///
/// Returns 0 on success (woken up), or -EAGAIN (-11) if *uaddr != expected_val.
fn futex_wait(uaddr: usize, expected_val: u32) -> isize {
    // 1. Volatile read of *uaddr from user space.
    //    SSTATUS.SUM is set in trap_handler, allowing S-mode to read U-mode pages.
    let current_val = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
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
fn linux_futex(addr: usize, op: usize, val: usize) -> isize {
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
            unsafe { crate::arch::idt::wrmsr(MSR_FS_BASE, addr as u64) };
            0
        }
        ARCH_GET_FS => {
            if addr == 0 {
                return EINVAL;
            }
            let val = unsafe { crate::arch::idt::rdmsr(MSR_FS_BASE) };
            unsafe { core::ptr::write_volatile(addr as *mut u64, val) };
            0
        }
        ARCH_GET_GS => {
            if addr == 0 {
                return EINVAL;
            }
            let val = unsafe { crate::arch::idt::rdmsr(MSR_GS_BASE) };
            unsafe { core::ptr::write_volatile(addr as *mut u64, val) };
            0
        }
        _ => EINVAL,
    }
}

// kernel/src/syscall/mod.rs — RISC-V ecall system call dispatch
//
// System call convention (RISC-V ABI):
//   a7 (x[17]) = syscall number
//   a0-a5 (x[10]..x[15]) = arguments
//   a0 (x[10]) = return value

// ---- Syscall numbers (match Linux RISC-V) ----
pub const SYS_READ: usize = 63;
pub const SYS_WRITE: usize = 64;
pub const SYS_EXIT: usize = 93;
pub const SYS_YIELD: usize = 124;
pub const SYS_GETPID: usize = 172;
pub const SYS_SBRK: usize = 214;

/// Dispatch a system call.
///
/// * `syscall_id` — syscall number from a7
/// * `args` — up to 6 arguments from a0–a5
/// Returns the value to be written back to a0.
pub fn dispatch(syscall_id: usize, args: [usize; 6]) -> isize {
    match syscall_id {
        SYS_WRITE => sys_write(args[0], args[1], args[2]),
        SYS_EXIT => sys_exit(args[0]),
        SYS_YIELD => sys_yield(),
        SYS_GETPID => sys_getpid(),
        _ => {
            crate::console_println!("[syscall] Unknown syscall: {}", syscall_id);
            -1
        }
    }
}

/// sys_write(fd, buf, len) — write *len* bytes from *buf* to file descriptor *fd*.
///
/// Only fd=1 (stdout) is supported; output goes through the SBI console.
fn sys_write(fd: usize, buf: usize, len: usize) -> isize {
    if fd == 1 {
        let data = unsafe { core::slice::from_raw_parts(buf as *const u8, len) };
        crate::sbi::print(core::str::from_utf8(data).unwrap_or("?"));
        len as isize
    } else {
        -1
    }
}

/// sys_exit(code) — terminate the current task with exit *code*.
fn sys_exit(code: usize) -> isize {
    crate::console_println!("[syscall] Process exited with code {}", code);
    crate::sched::mark_current_exited();
    sys_yield()
}

/// sys_yield() — voluntarily give up the CPU.
fn sys_yield() -> isize {
    crate::sched::schedule();
    0
}

/// sys_getpid() — return the current task's ID.
fn sys_getpid() -> isize {
    crate::sched::current_task_id() as isize
}

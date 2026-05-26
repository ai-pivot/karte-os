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

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── Syscall Tests ──");

    // Test 1: Unknown syscall returns -1
    crate::test::run_test("syscall_unknown_returns_error", || {
        dispatch(9999, [0, 0, 0, 0, 0, 0]) == -1
    });

    // Test 2: sys_getpid returns valid task id
    crate::test::run_test("syscall_getpid_returns_valid", || {
        let pid = dispatch(SYS_GETPID, [0, 0, 0, 0, 0, 0]);
        pid >= 0
    });

    // Test 3: sys_write to invalid fd returns -1
    crate::test::run_test("syscall_write_bad_fd_returns_error", || {
        let result = dispatch(SYS_WRITE, [0, 0, 0, 0, 0, 0]); // fd=0
        result == -1
    });

    // Test 4: sys_write to stdout succeeds
    crate::test::run_test("syscall_write_stdout_succeeds", || {
        // Write "Hi" to fd=1
        let msg = b"Hi";
        let result = dispatch(SYS_WRITE, [1, msg.as_ptr() as usize, msg.len(), 0, 0, 0]);
        result == 2
    });

    // Test 5: Syscall constants are correct
    crate::test::run_test("syscall_constants_correct", || {
        SYS_READ == 63
            && SYS_WRITE == 64
            && SYS_EXIT == 93
            && SYS_YIELD == 124
            && SYS_GETPID == 172
            && SYS_SBRK == 214
    });

    // Test 6: sys_yield returns 0
    crate::test::run_test("syscall_yield_returns_zero", || {
        dispatch(SYS_YIELD, [0, 0, 0, 0, 0, 0]) == 0
    });
}

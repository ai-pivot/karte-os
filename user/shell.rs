//! shell.rs — KarteOS interactive shell
//!
//! A minimal no_std Rust shell. Compiled as a freestanding RISC-V ELF.
//!
//! The kernel TTY subsystem handles:
//!   - Canonical mode line editing (echo, backspace, Ctrl+C)
//!   - Blocking sys_read (task sleeps until Enter is pressed)
//!   - Complete lines are returned via sys_read(fd=0)
//!
//! Build:
//!   rustc --target riscv64gc-unknown-none-elf -C link-arg=-Tuser.ld \
//!         -C opt-level=s -o shell.elf shell.rs

#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

// ─── Syscall numbers (KarteOS native ABI) ──────────────────────────

const SYS_EXIT: usize = 1;
const SYS_WRITE: usize = 2;
const SYS_READ: usize = 3;
const SYS_GETPID: usize = 5;
const SYS_SPAWN: usize = 30;
const SYS_WAITPID: usize = 31;
const SYS_LS: usize = 40;

/// sys_waitpid sentinel: child still running, poll again. Matches the kernel.
const WAIT_AGAIN: isize = -1;

// ─── Syscall wrapper (ecall) ───────────────────────────────────────

#[inline(always)]
unsafe fn syscall1(id: usize, a0: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "ecall",
        in("a7") id,
        inlateout("a0") a0 as usize => ret,
    );
    ret
}

#[inline(always)]
unsafe fn syscall2(id: usize, a0: usize, a1: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "ecall",
        in("a7") id,
        inlateout("a0") a0 => ret,
        in("a1") a1,
    );
    ret
}

#[inline(always)]
unsafe fn syscall3(id: usize, a0: usize, a1: usize, a2: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "ecall",
        in("a7") id,
        inlateout("a0") a0 => ret,
        in("a1") a1,
        in("a2") a2,
    );
    ret
}

// ─── Helper functions ──────────────────────────────────────────────

fn print(s: &[u8]) {
    unsafe { syscall3(SYS_WRITE, 1, s.as_ptr() as usize, s.len()); }
}

fn println(s: &[u8]) {
    print(s);
    print(b"\r\n");
}

fn print_u64(mut n: u64) {
    if n == 0 {
        print(b"0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    let mut j = i;
    while j > 0 {
        j -= 1;
        print(&[buf[j]]);
    }
}

/// Read a line from stdin (polls with yield).
/// The kernel TTY returns complete lines ending with \n.
/// Returns the line length (including \n), or 0 if no data.
fn read_line(buf: &mut [u8]) -> usize {
    loop {
        let n = unsafe { syscall3(SYS_READ, 0, buf.as_ptr() as usize, buf.len()) };
        if n > 0 {
            return n as usize;
        }
        // No data yet — yield CPU via ecall (any ecall will do, timer will poll UART)
        // Brief spin to avoid excessive syscall overhead
        for _ in 0..1000 {
            core::hint::spin_loop();
        }
    }
}

/// Compare byte slices
fn str_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    for i in 0..a.len() {
        if a[i] != b[i] { return false; }
    }
    true
}

/// Check if slice starts with prefix, return remainder
fn str_strip_prefix<'a>(s: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if s.len() >= prefix.len() && &s[..prefix.len()] == prefix {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Trim trailing \n, \r, spaces
fn trim_right(s: &[u8]) -> &[u8] {
    let mut end = s.len();
    while end > 0 && (s[end - 1] == b'\n' || s[end - 1] == b'\r' || s[end - 1] == b' ') {
        end -= 1;
    }
    &s[..end]
}

/// Trim leading spaces
fn trim_left(s: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < s.len() && s[start] == b' ' {
        start += 1;
    }
    &s[start..]
}

fn trim(s: &[u8]) -> &[u8] {
    trim_right(trim_left(s))
}

// ─── Built-in commands ──────────────────────────────────────────────

fn cmd_help() {
    println(b"KarteOS Shell v0.2");
    println(b"");
    println(b"Commands:");
    println(b"  help          - show this help");
    println(b"  ls            - list filesystem");
    println(b"  cat <file>    - show file contents");
    println(b"  echo <text>   - print text");
    println(b"  run <prog>    - run embedded program");
    println(b"  spawn <n>     - spawn program by number");
    println(b"  pid           - show process ID");
    println(b"  clear         - clear screen");
    println(b"  exit          - exit shell");
    println(b"");
    println(b"Line editing: Backspace, Ctrl+C supported by kernel TTY");
}

fn cmd_ls() {
    let buf = [0u8; 4096];
    let n = unsafe { syscall2(SYS_LS, buf.as_ptr() as usize, buf.len()) };
    if n > 0 {
        print(&buf[..n as usize]);
    }
}

fn cmd_cat(path: &[u8]) {
    if path.is_empty() {
        println(b"Usage: cat <file>");
        return;
    }
    let fd = unsafe { syscall3(10 /*SYS_OPEN*/, path.as_ptr() as usize, path.len(), 0) };
    if fd < 0 {
        print(b"cat: file not found: ");
        println(path);
        return;
    }
    let buf = [0u8; 512];
    loop {
        let n = unsafe { syscall3(SYS_READ, fd as usize, buf.as_ptr() as usize, buf.len()) };
        if n <= 0 { break; }
        print(&buf[..n as usize]);
    }
    unsafe { syscall2(11 /*SYS_CLOSE*/, fd as usize, 0) };
}

fn cmd_echo(text: &[u8]) {
    println(text);
}

fn cmd_run(path: &[u8]) {
    if path.is_empty() {
        println(b"Usage: run <file>");
        return;
    }
    let prog_id: usize = match path {
        b"/hello" | b"hello" => 0,
        b"/heap_test" | b"heap_test" => 1,
        b"/file_test" | b"file_test" => 2,
        b"/spawn_test" | b"spawn_test" => 3,
        _ => {
            print(b"run: unknown program: ");
            println(path);
            println(b"  available: hello, heap_test, file_test, spawn_test");
            return;
        }
    };
    let pid = unsafe { syscall2(SYS_SPAWN, prog_id, 0) };
    if pid < 0 {
        println(b"run: failed to spawn");
        return;
    }
    print(b"spawned pid=");
    print_u64(pid as u64);
    println(b"");

    // Wait for child to finish (polling waitpid).
    //   WAIT_AGAIN (-1) → still running, poll again
    //   < -1            → error (not our child)
    //   >= 0            → exited with this code
    loop {
        let code = unsafe { syscall1(SYS_WAITPID, pid as usize) };
        if code == WAIT_AGAIN {
            for _ in 0..100 {
                core::hint::spin_loop();
            }
            continue;
        }
        if code < 0 {
            println(b"run: waitpid failed");
            return;
        }
        print(b"[run] process ");
        print_u64(pid as u64);
        print(b" exited with code ");
        print_u64(code as u64);
        println(b"");
        return;
    }
}

fn cmd_spawn(id: &[u8]) {
    if id.is_empty() {
        println(b"Usage: spawn <0-3>");
        println(b"  0=hello  1=heap_test  2=file_test  3=spawn_test");
        return;
    }
    let prog_id = if id.len() == 1 && id[0] >= b'0' && id[0] <= b'9' {
        (id[0] - b'0') as usize
    } else {
        99
    };
    let pid = unsafe { syscall2(SYS_SPAWN, prog_id, 0) };
    if pid < 0 {
        println(b"spawn: failed");
        return;
    }
    print(b"spawned pid=");
    print_u64(pid as u64);
    println(b"");
    // Poll-wait (non-blocking waitpid); see cmd_run for the return convention.
    let exit_code = loop {
        let code = unsafe { syscall1(SYS_WAITPID, pid as usize) };
        if code == WAIT_AGAIN {
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
            continue;
        }
        if code < 0 {
            println(b"spawn: waitpid failed");
            return;
        }
        break code;
    };
    print(b"child exited with code ");
    print_u64(exit_code as u64);
    println(b"");
}

fn cmd_pid() {
    let pid = unsafe { syscall1(SYS_GETPID, 0) };
    print(b"pid: ");
    print_u64(pid as u64);
    println(b"");
}

fn cmd_clear() {
    print(b"\x1b[2J\x1b[H");
}

// ─── Entry point ────────────────────────────────────────────────────

const LINE_BUF_SIZE: usize = 256;

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    println(b"");
    println(b"  ====================================");
    println(b"  |      KarteOS Shell v0.2          |");
    println(b"  |  Type 'help' for commands        |");
    println(b"  ====================================");
    println(b"");

    let mut line_buf = [0u8; LINE_BUF_SIZE];

    loop {
        // Print prompt
        print(b"$ ");

        // Read a complete line (blocking — kernel TTY handles line editing)
        let len = read_line(&mut line_buf);
        if len == 0 {
            continue;
        }

        // Trim the line (remove trailing \n, \r, spaces)
        let cmd = trim(&line_buf[..len]);
        if cmd.is_empty() {
            continue;
        }

        // Dispatch command
        if str_eq(cmd, b"help") {
            cmd_help();
        } else if str_eq(cmd, b"ls") {
            cmd_ls();
        } else if str_eq(cmd, b"pid") {
            cmd_pid();
        } else if str_eq(cmd, b"clear") {
            cmd_clear();
        } else if str_eq(cmd, b"exit") || str_eq(cmd, b"quit") {
            println(b"bye!");
            syscall1(SYS_EXIT, 0);
        } else if let Some(text) = str_strip_prefix(cmd, b"echo ") {
            cmd_echo(text);
        } else if let Some(text) = str_strip_prefix(cmd, b"echo") {
            cmd_echo(text);
        } else if let Some(path) = str_strip_prefix(cmd, b"cat ") {
            cmd_cat(trim(path));
        } else if let Some(path) = str_strip_prefix(cmd, b"run ") {
            cmd_run(trim(path));
        } else if let Some(id) = str_strip_prefix(cmd, b"spawn ") {
            cmd_spawn(trim(id));
        } else {
            print(b"unknown command: ");
            println(cmd);
            println(b"type 'help' for available commands");
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    print(b"PANIC!\r\n");
    loop {}
}

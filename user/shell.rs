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
const SYS_OPEN: usize = 10;
const SYS_CLOSE: usize = 11;
const SYS_SPAWN: usize = 30;
const SYS_WAITPID: usize = 31;
const SYS_EXEC: usize = 32;
const SYS_LS: usize = 40;
const SYS_MKDIR: usize = 41;
const SYS_UNLINK: usize = 42;
const SYS_SETENV: usize = 50;
const SYS_GETENV: usize = 51;

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

#[inline(always)]
unsafe fn syscall4(id: usize, a0: usize, a1: usize, a2: usize, a3: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "ecall",
        in("a7") id,
        inlateout("a0") a0 => ret,
        in("a1") a1,
        in("a2") a2,
        in("a3") a3,
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

/// Current working directory (shell state).
static mut CWD: [u8; 256] = [b'/'; 256];
static mut CWD_LEN: usize = 1; // "/"

fn get_cwd() -> &'static [u8] {
    unsafe { &CWD[..CWD_LEN] }
}

fn set_cwd(path: &[u8]) {
    unsafe {
        let len = if path.len() < 256 { path.len() } else { 255 };
        CWD[..len].copy_from_slice(&path[..len]);
        CWD_LEN = len;
    }
}

/// Resolve a path against CWD. If path starts with '/', use as-is.
fn resolve_path<'a>(path: &'a [u8], resolved: &'a mut [u8]) -> &'a [u8] {
    if path.starts_with(b"/") {
        // Strip leading '/'
        let p = if path.len() > 1 { &path[1..] } else { path };
        return p;
    }
    // Relative path: prepend CWD (without leading /)
    let cwd = get_cwd();
    let cwd_stripped = if cwd.starts_with(b"/") { &cwd[1..] } else { cwd };
    let mut pos = 0;
    for &b in cwd_stripped {
        if pos < 255 { resolved[pos] = b; pos += 1; }
    }
    if pos > 0 && pos < 255 { resolved[pos] = b'/'; pos += 1; }
    for &b in path {
        if pos < 255 { resolved[pos] = b; pos += 1; }
    }
    &resolved[..pos]
}

fn cmd_help() {
    println(b"KarteOS Shell v0.2");
    println(b"");
    println(b"Commands:");
    println(b"  help          - show this help");
    println(b"  ls            - list filesystem");
    println(b"  cat <file>    - show file contents");
    println(b"  echo <text>   - print text");
    println(b"  cd  <dir>     - change directory");
    println(b"  mkdir <dir>   - create directory");
    println(b"  rm   <file>   - remove file/directory");
    println(b"  run  <prog>   - run program (from PATH)");
    println(b"  spawn <n>     - spawn program by number");
    println(b"  pid           - show process ID");
    println(b"  env           - show environment");
    println(b"  clear         - clear screen");
    println(b"  exit          - exit shell");
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

    let mut resolved = [0u8; 256];
    let p = resolve_path(path, &mut resolved);
    let pid = cmd_exec(p);

    if pid >= 0 {
        // Wait for child
        loop {
            let code = unsafe { syscall1(SYS_WAITPID, pid as usize) };
            if code == WAIT_AGAIN {
                for _ in 0..1000 {
                    core::hint::spin_loop();
                }
                continue;
            }
            if code < 0 {
                println(b"run: waitpid failed");
                return;
            }
            return;
        }
    }

    // Legacy fallback: try known program IDs
    let prog_id: usize = match path {
        b"/hello" | b"hello" => 0,
        b"/heap_test" | b"heap_test" => 1,
        b"/file_test" | b"file_test" => 2,
        b"/spawn_test" | b"spawn_test" => 3,
        _ => {
            print(b"run: file not found: ");
            println(path);
            return;
        }
    };
    let pid2 = unsafe { syscall2(SYS_SPAWN, prog_id, 0) };
    if pid2 < 0 {
        println(b"run: failed to spawn");
        return;
    }
    wait_and_report(pid2);
}

/// Wait for a child process and report its exit status.
fn wait_and_report(pid: isize) {
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

fn cmd_cd(path: &[u8]) {
    if path.is_empty() {
        // cd with no args: show current directory
        print(get_cwd());
        print(b"\r\n");
        return;
    }
    // Simple cd: if path starts with letter (not /), treat as relative to cwd
    if path.starts_with(b"/") {
        set_cwd(path);
    } else {
        let mut resolved = [0u8; 256];
        let p = resolve_path(path, &mut resolved);
        let mut c = [0u8; 256];
        c[0] = b'/';
        let len = p.len().min(255);
        c[1..1+len].copy_from_slice(&p[..len]);
        set_cwd(&c[..1+len]);
    }
}

fn cmd_mkdir(path: &[u8]) {
    if path.is_empty() {
        println(b"Usage: mkdir <dir>");
        return;
    }
    let mut resolved = [0u8; 256];
    let p = resolve_path(path, &mut resolved);
    let r = unsafe { syscall2(SYS_MKDIR, p.as_ptr() as usize, p.len()) };
    if r < 0 {
        print(b"mkdir: failed: ");
        println(p);
    }
}

fn cmd_rm(path: &[u8]) {
    if path.is_empty() {
        println(b"Usage: rm <file>");
        return;
    }
    let mut resolved = [0u8; 256];
    let p = resolve_path(path, &mut resolved);
    let r = unsafe { syscall2(SYS_UNLINK, p.as_ptr() as usize, p.len()) };
    if r < 0 {
        print(b"rm: failed: ");
        println(p);
    }
}

fn cmd_env() {
    // Read PATH env var
    let mut buf = [0u8; 256];
    let n = unsafe { syscall4(SYS_GETENV, b"PATH".as_ptr() as usize, 4, buf.as_ptr() as usize, buf.len()) };
    if n > 0 {
        print(b"PATH=");
        print(&buf[..n as usize]);
        println(b"");
    } else {
        println(b"PATH=/");
    }
    let n2 = unsafe { syscall4(SYS_GETENV, b"USER".as_ptr() as usize, 4, buf.as_ptr() as usize, buf.len()) };
    if n2 > 0 {
        print(b"USER=");
        print(&buf[..n2 as usize]);
        println(b"");
    }
    // Show CWD
    print(b"CWD=");
    print(get_cwd());
    println(b"");
}

/// Append two byte slices.
fn append_slice(dst: &mut [u8], pos: &mut usize, src: &[u8]) {
    for &b in src {
        if *pos < dst.len() {
            dst[*pos] = b;
            *pos += 1;
        }
    }
}

/// Try to run a binary by name, searching PATH directories.
fn cmd_exec(name: &[u8]) -> isize {
    if name.is_empty() {
        return -1;
    }
    // Try exact name first (sys_exec handles absolute/relative paths)
    let pid = unsafe { syscall2(SYS_EXEC, name.as_ptr() as usize, name.len()) };
    if pid >= 0 {
        return pid;
    }
    // If name doesn't start with /, search PATH
    if !name.starts_with(b"/") {
        let mut path_buf = [0u8; 512];
        let n = unsafe { syscall4(SYS_GETENV, b"PATH".as_ptr() as usize, 4, path_buf.as_ptr() as usize, path_buf.len()) };
        if n > 0 {
            let path_str = &path_buf[..n as usize];
            // Split PATH by ':'
            let mut start = 0;
            loop {
                if start >= path_str.len() {
                    break;
                }
                let mut end = start;
                while end < path_str.len() && path_str[end] != b':' {
                    end += 1;
                }
                let dir = &path_str[start..end];
                // Build: <dir>/<name>
                let mut full = [0u8; 512];
                let mut pos = 0;
                append_slice(&mut full, &mut pos, dir);
                if pos > 0 && pos < 511 { full[pos] = b'/'; pos += 1; }
                append_slice(&mut full, &mut pos, name);
                let full_path = &full[..pos];
                let pid2 = unsafe { syscall2(SYS_EXEC, full_path.as_ptr() as usize, full_path.len()) };
                if pid2 >= 0 {
                    return pid2;
                }
                start = end + 1;
            }
        }
    }
    -1
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
        } else if str_eq(cmd, b"env") {
            cmd_env();
        } else if str_eq(cmd, b"exit") || str_eq(cmd, b"quit") {
            println(b"bye!");
            syscall1(SYS_EXIT, 0);
        } else if let Some(text) = str_strip_prefix(cmd, b"echo ") {
            cmd_echo(text);
        } else if let Some(text) = str_strip_prefix(cmd, b"echo") {
            cmd_echo(text);
        } else if let Some(path) = str_strip_prefix(cmd, b"cat ") {
            cmd_cat(trim(path));
        } else if let Some(path) = str_strip_prefix(cmd, b"cd ") {
            cmd_cd(trim(path));
        } else if let Some(path) = str_strip_prefix(cmd, b"mkdir ") {
            cmd_mkdir(trim(path));
        } else if let Some(path) = str_strip_prefix(cmd, b"rm ") {
            cmd_rm(trim(path));
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

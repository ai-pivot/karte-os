//! shell.rs — KarteOS interactive shell (launcher-style)
//!
//! Only cd/exit/export are built-in. All other commands are separate
//! binaries found via PATH (/bin by default).
//!
//! Argument passing: shell sets CMD_ARGS env var before spawning.

#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

const SYS_SPAWN: usize = 30;
const SYS_WAITPID: usize = 31;
const WAIT_AGAIN: isize = -1;
const LINE_BUF_SIZE: usize = 256;

// ── Shell state ──
static mut CWD: [u8; 256] = [0u8; 256];
static mut CWD_LEN: usize = 0;

fn get_cwd() -> &'static [u8] {
    let ptr = core::ptr::addr_of!(CWD);
    let len = unsafe { CWD_LEN };
    unsafe { core::slice::from_raw_parts(ptr as *const u8, len) }
}

fn update_cwd(path: &[u8]) {
    let len = path.len().min(255);
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), core::ptr::addr_of_mut!(CWD) as *mut u8, len);
        CWD_LEN = len;
    }
    // Sync to kernel env so pwd can read it
    unsafe {
        syscall4(SYS_SETENV, b"CWD".as_ptr() as usize, 3, 
            core::ptr::addr_of!(CWD) as usize, CWD_LEN);
    }
}

/// Resolve a path: if absolute, strip '/'. If relative, prepend CWD.
fn resolve<'a>(path: &'a [u8], buf: &'a mut [u8]) -> &'a [u8] {
    if path.starts_with(b"/") {
        let p = if path.len() > 1 { &path[1..] } else { &b""[..] };
        return p;
    }
    let cwd = get_cwd();
    let mut pos = 0;
    for &b in cwd {
        if pos < buf.len() { buf[pos] = b; pos += 1; }
    }
    if pos > 0 && pos < buf.len() { buf[pos] = b'/'; pos += 1; }
    for &b in path {
        if pos < buf.len() { buf[pos] = b; pos += 1; }
    }
    &buf[..pos]
}

/// Read a line from stdin.
fn read_line(buf: &mut [u8]) -> usize {
    loop {
        let n = unsafe { syscall3(SYS_READ, 0, buf.as_ptr() as usize, buf.len()) };
        if n > 0 { return n as usize; }
        for _ in 0..1000 { core::hint::spin_loop(); }
    }
}

/// Wait for a child PID.
fn wait_for(pid: isize) {
    loop {
        let code = unsafe { syscall1(SYS_WAITPID, pid as usize) };
        if code == WAIT_AGAIN {
            for _ in 0..1000 { core::hint::spin_loop(); }
            continue;
        }
        return;
    }
}

/// Search PATH and try to exec a command. Returns pid or -1.
fn launch(cmd: &[u8], arg: &[u8]) -> isize {
    // Set CMD_ARGS so the child can read its arguments
    unsafe {
        syscall4(SYS_SETENV, b"CMD_ARGS".as_ptr() as usize, 8, arg.as_ptr() as usize, arg.len());
    }

    // 1. Try exact path (absolute or relative)
    let pid = unsafe { syscall2(SYS_EXEC, cmd.as_ptr() as usize, cmd.len()) };
    if pid >= 0 { return pid; }

    // 2. If not found and not an absolute path, search PATH
    if !cmd.starts_with(b"/") && !cmd.starts_with(b"./") {
        if let Some(path_buf) = getenv(b"PATH") {
            let path_str = &path_buf;
            let mut start = 0;
            loop {
                if start >= path_str.len() || path_str[start] == 0 { break; }
                let mut end = start;
                while end < path_str.len() && path_str[end] != b':' && path_str[end] != 0 { end += 1; }
                let dir = &path_str[start..end];

                // Build <dir>/<cmd>
                let mut full = [0u8; 512];
                let mut pos = 0;
                for &b in dir {
                    if pos < 511 { full[pos] = b; pos += 1; }
                }
                if pos > 0 && pos < 511 { full[pos] = b'/'; pos += 1; }
                for &b in cmd {
                    if pos < 511 { full[pos] = b; pos += 1; }
                }
                let full_path = &full[..pos];
                let pid2 = unsafe { syscall2(SYS_EXEC, full_path.as_ptr() as usize, full_path.len()) };
                if pid2 >= 0 { return pid2; }

                start = if end < path_str.len() && path_str[end] == b':' { end + 1 } else { end };
                if end >= path_str.len() || path_str[end] == 0 { break; }
            }
        }
    }

    // 3. Legacy: try RamFS embedded programs by name
    let pid3 = unsafe { syscall2(SYS_EXEC, cmd.as_ptr() as usize, cmd.len()) };
    if pid3 >= 0 { return pid3; }

    -1
}

// ── Built-in: cd ──
fn builtin_cd(arg: &[u8]) {
    let arg = trim(arg);
    if arg.is_empty() {
        print(b"/\r\n");
        return;
    }
    if arg.starts_with(b"/") {
        update_cwd(arg);
    } else {
        let cwd = get_cwd();
        let mut new_path = [0u8; 256];
        let mut pos = 0;
        for &b in cwd {
            if pos < 255 { new_path[pos] = b; pos += 1; }
        }
        if pos > 0 && pos < 255 && new_path[pos - 1] != b'/' {
            new_path[pos] = b'/';
            pos += 1;
        }
        for &b in arg {
            if pos < 255 { new_path[pos] = b; pos += 1; }
        }
        update_cwd(&new_path[..pos]);
    }
}

// ── Built-in: export ──
fn builtin_export(arg: &[u8]) {
    let arg = trim(arg);
    // Parse KEY=VALUE
    if let Some(eq) = arg.iter().position(|&b| b == b'=') {
        let key = &arg[..eq];
        let val = &arg[eq+1..];
        unsafe {
            syscall4(SYS_SETENV, key.as_ptr() as usize, key.len(), val.as_ptr() as usize, val.len());
        }
    } else {
        // Print the variable
        match getenv(arg) {
            Some(buf) => {
                let len = buf.iter().position(|&b| b == 0).unwrap_or(0);
                if len > 0 { print(&buf[..len]); println(b""); }
            }
            None => { print(arg); println(b" not found"); }
        }
    }
}

// ── Entry ──
#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    // Init CWD to "/"
    update_cwd(b"/");

    println(b"");
    println(b"  ====================================");
    println(b"  |      KarteOS Shell v0.3          |");
    println(b"  |  Type 'help' for commands        |");
    println(b"  ====================================");
    println(b"");

    let mut line_buf = [0u8; LINE_BUF_SIZE];

    loop {
        print(b"$ ");
        let len = read_line(&mut line_buf);
        if len == 0 { continue; }

        let cmd = trim(&line_buf[..len]);
        if cmd.is_empty() { continue; }

        // Split into command name and rest
        let (name, rest) = split_first(cmd);
        let name = trim(name);
        let arg = trim(rest);

        // Built-in: exit
        if name == b"exit" || name == b"quit" {
            println(b"bye!");
            syscall1(SYS_EXIT, 0);
        }
        // Built-in: cd
        else if name == b"cd" {
            builtin_cd(arg);
        }
        // Built-in: export
        else if name == b"export" {
            builtin_export(arg);
        }
        // Built-in: help
        else if name == b"help" {
            println(b"KarteOS Shell v0.3");
            println(b"Built-in: cd, exit, export, help");
            println(b"Other commands are loaded from PATH (/bin)");
        }
        // Launch binary
        else {
            let pid = launch(name, arg);
            if pid >= 0 {
                wait_for(pid);
            } else {
                print(b"command not found: ");
                println(name);
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

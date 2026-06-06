//! shell.rs — KarteOS interactive shell v0.5
//!
//! Features:
//!   - Pipe: cmd1 | cmd2 | cmd3
//!   - Redirect: cmd > file, cmd >> file, cmd < file
//!   - Command history: ↑/↓ to browse, Ctrl+C to cancel
//!   - Tab completion: lists matching PATH binaries
//!   - Built-ins: cd, exit, export, help, kill

#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

const SYS_SPAWN: usize = 30;
const SYS_WAITPID: usize = 31;
const WAIT_AGAIN: isize = -1;
const LINE_BUF_SIZE: usize = 512;
const HISTORY_SIZE: usize = 64;
const MAX_CMDS_IN_PIPE: usize = 8;

unsafe fn update_cwd(path: &[u8]) {
    let mut cwd = [0u8; 256];
    let len = path.len().min(255);
    core::ptr::copy_nonoverlapping(path.as_ptr(), cwd.as_mut_ptr(), len);
    syscall4(SYS_SETENV, b"CWD".as_ptr() as usize, 3, cwd.as_ptr() as usize, len);
}

unsafe fn history_add(line: &[u8]) {
    static mut HISTORY: [[u8; LINE_BUF_SIZE]; HISTORY_SIZE] = [[0; LINE_BUF_SIZE]; HISTORY_SIZE];
    static mut HIST_HEAD: usize = 0;
    static mut HIST_COUNT: usize = 0;
    static mut HIST_VIEW: usize = 0;
    if line.is_empty() { return; }
    let len = line.len().min(LINE_BUF_SIZE - 1);
    let slot = HIST_HEAD % HISTORY_SIZE;
    core::ptr::copy_nonoverlapping(line.as_ptr(), HISTORY[slot].as_mut_ptr(), len);
    HISTORY[slot][len] = 0;
    HIST_HEAD = HIST_HEAD + 1;
    if HIST_COUNT < HISTORY_SIZE { HIST_COUNT += 1; }
    HIST_VIEW = HIST_HEAD;
}

unsafe fn history_get(offset: usize) -> usize {
    static mut HISTORY: [[u8; LINE_BUF_SIZE]; HISTORY_SIZE] = [[0; LINE_BUF_SIZE]; HISTORY_SIZE];
    static mut HIST_HEAD: usize = 0;
    static mut HIST_COUNT: usize = 0;
    if offset >= HIST_COUNT { return 0; }
    let slot = (HIST_HEAD + HISTORY_SIZE - 1 - offset) % HISTORY_SIZE;
    HISTORY[slot].iter().position(|&b| b == 0).unwrap_or(0)
}

unsafe fn read_line(buf: &mut [u8]) -> usize {
    static mut HIST_VIEW: usize = 0;
    let mut pos = 0usize;

    loop {
        let mut byte = [0u8; 1];
        let n = syscall3(SYS_READ, 0, byte.as_mut_ptr() as usize, 1);
        if n <= 0 { continue; }
        let c = byte[0];

        match c {
            b'\r' | b'\n' => {
                print(b"\r\n");
                buf[pos] = 0;
                break;
            }
            0x08 | 0x7F => {
                if pos > 0 {
                    pos -= 1;
                    print(b"\x08 \x08");
                }
            }
            0x03 => {
                print(b"^C\r\n");
                pos = 0;
                buf[0] = 0;
                break;
            }
            0x1B => {
                let mut seq = [0u8; 2];
                let n1 = syscall3(SYS_READ, 0, seq.as_mut_ptr() as usize, 1);
                if n1 <= 0 { continue; }
                let n2 = syscall3(SYS_READ, 0, seq.as_mut_ptr().add(1) as usize, 1);
                if n2 <= 0 { continue; }
                if seq[0] == b'[' {
                    match seq[1] {
                        b'A' => {
                            if HIST_VIEW > 0 { HIST_VIEW -= 1; }
                            for _ in 0..pos { print(b"\x08 \x08"); }
                            pos = history_get(HIST_VIEW);
                            if pos > 0 {
                                let entry = history_get_entry(HIST_VIEW);
                                core::ptr::copy_nonoverlapping(entry.as_ptr(), buf.as_mut_ptr(), pos);
                                print(&buf[..pos]);
                            }
                        }
                        b'B' => {
                            HIST_VIEW += 1;
                            for _ in 0..pos { print(b"\x08 \x08"); }
                            let hist_count = history_count();
                            if HIST_VIEW >= hist_count {
                                HIST_VIEW = hist_count;
                                pos = 0;
                            } else {
                                pos = history_get(HIST_VIEW);
                                if pos > 0 {
                                    let entry = history_get_entry(HIST_VIEW);
                                    core::ptr::copy_nonoverlapping(entry.as_ptr(), buf.as_mut_ptr(), pos);
                                    print(&buf[..pos]);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            0x09 => {
                // Tab — basic echo of newline + re-prompt
                print(b"\r\n$ ");
                print(&buf[..pos]);
            }
            0x20..=0x7E => {
                if pos < buf.len() - 1 {
                    buf[pos] = c;
                    pos += 1;
                    print(&[c]);
                }
            }
            _ => {}
        }
    }
    pos
}

unsafe fn history_get_entry(offset: usize) -> [u8; LINE_BUF_SIZE] {
    static mut HISTORY: [[u8; LINE_BUF_SIZE]; HISTORY_SIZE] = [[0; LINE_BUF_SIZE]; HISTORY_SIZE];
    static mut HIST_HEAD: usize = 0;
    let mut out = [0u8; LINE_BUF_SIZE];
    let slot = (HIST_HEAD + HISTORY_SIZE - 1 - offset) % HISTORY_SIZE;
    out.copy_from_slice(&HISTORY[slot]);
    out
}

unsafe fn history_count() -> usize {
    static mut HIST_COUNT: usize = 0;
    HIST_COUNT
}

unsafe fn reset_hist_view() {
    static mut HIST_VIEW: usize = 0;
    HIST_VIEW = history_count();
}

fn wait_for(pid: isize) -> i32 {
    loop {
        let code = unsafe { syscall1(SYS_WAITPID, pid as usize) };
        if code == WAIT_AGAIN {
            for _ in 0..1000 { core::hint::spin_loop(); }
            continue;
        }
        return code as i32;
    }
}

unsafe fn launch(cmd: &[u8], arg: &[u8], redir_stdin: i32, redir_stdout: i32) -> isize {
    syscall4(SYS_SETENV, b"CMD_ARGS".as_ptr() as usize, 8, arg.as_ptr() as usize, arg.len());

    if redir_stdin >= 0 || redir_stdout >= 0 {
        // Try exec with fd redirection
        let pid = syscall4(SYS_EXEC_FD, cmd.as_ptr() as usize, cmd.len(), redir_stdin as usize, redir_stdout as usize);
        if pid >= 0 { return pid; }
        // PATH search
        let pid = search_path_exec_fd(cmd, redir_stdin, redir_stdout);
        if pid >= 0 { return pid; }
        return -1;
    }

    // Try exact path
    let pid = syscall2(SYS_EXEC, cmd.as_ptr() as usize, cmd.len());
    if pid >= 0 { return pid; }

    // PATH search
    let pid = search_path_exec(cmd);
    if pid >= 0 { return pid; }

    -1
}

unsafe fn search_path_exec_fd(cmd: &[u8], redir_stdin: i32, redir_stdout: i32) -> isize {
    let path_buf = match getenv(b"PATH") {
        Some(b) => b,
        None => return -1,
    };
    let mut start = 0;
    let len = path_buf.iter().position(|&b| b == 0).unwrap_or(path_buf.len());
    loop {
        if start >= len { break; }
        let mut end = start;
        while end < len && path_buf[end] != b':' { end += 1; }
        let dir = &path_buf[start..end];

        let mut full = [0u8; 512];
        let mut p = 0;
        for &b in dir { if p < 511 { full[p] = b; p += 1; } }
        if p > 0 && p < 511 { full[p] = b'/'; p += 1; }
        for &b in cmd { if p < 511 { full[p] = b; p += 1; } }
        let pid = syscall4(SYS_EXEC_FD, full.as_ptr() as usize, p, redir_stdin as usize, redir_stdout as usize);
        if pid >= 0 { return pid; }

        start = if end < len && path_buf[end] == b':' { end + 1 } else { end };
    }
    -1
}

unsafe fn search_path_exec(cmd: &[u8]) -> isize {
    let path_buf = match getenv(b"PATH") {
        Some(b) => b,
        None => return -1,
    };
    let mut start = 0;
    let len = path_buf.iter().position(|&b| b == 0).unwrap_or(path_buf.len());
    loop {
        if start >= len { break; }
        let mut end = start;
        while end < len && path_buf[end] != b':' { end += 1; }
        let dir = &path_buf[start..end];

        let mut full = [0u8; 512];
        let mut p = 0;
        for &b in dir { if p < 511 { full[p] = b; p += 1; } }
        if p > 0 && p < 511 { full[p] = b'/'; p += 1; }
        for &b in cmd { if p < 511 { full[p] = b; p += 1; } }
        let pid = syscall2(SYS_EXEC, full.as_ptr() as usize, p);
        if pid >= 0 { return pid; }

        start = if end < len && path_buf[end] == b':' { end + 1 } else { end };
    }
    -1
}

unsafe fn builtin_cd(arg: &[u8]) {
    let arg = trim(arg);
    if arg.is_empty() { println(b"/"); return; }
    let mut target = [0u8; 256];
    let t = if arg.starts_with(b"/") {
        let l = arg.len().min(255);
        target[..l].copy_from_slice(&arg[..l]);
        &target[..l]
    } else {
        let cwd = match getenv(b"CWD") {
            Some(b) => {
                let l = b.iter().position(|&x| x == 0).unwrap_or(0);
                let l = l.min(255);
                target[..l].copy_from_slice(&b[..l]);
                l
            }
            None => 0,
        };
        let mut pos = cwd;
        if pos > 0 && pos < 255 && target[pos - 1] != b'/' {
            target[pos] = b'/'; pos += 1;
        }
        for &b in arg { if pos < 255 { target[pos] = b; pos += 1; } }
        let t = &target[..pos];
        t
    };
    let r = syscall2(SYS_CHDIR, t.as_ptr() as usize, t.len());
    if r < 0 {
        print(b"cd: no such directory: ");
        println(arg);
        return;
    }
    update_cwd(t);
}

unsafe fn builtin_export(arg: &[u8]) {
    let arg = trim(arg);
    if let Some(eq) = arg.iter().position(|&b| b == b'=') {
        let key = &arg[..eq];
        let val = &arg[eq + 1..];
        syscall4(SYS_SETENV, key.as_ptr() as usize, key.len(), val.as_ptr() as usize, val.len());
    } else {
        match getenv(arg) {
            Some(buf) => {
                let l = buf.iter().position(|&b| b == 0).unwrap_or(0);
                if l > 0 { print(&buf[..l]); println(b""); }
            }
            None => { print(arg); println(b" not found"); }
        }
    }
}

/// Scan for redirections (> >> <) in a command string.
/// Returns (clean_cmd, in_file, out_file, append_mode).
fn parse_redirects(cmd: &[u8]) -> (&[u8], &[u8], &[u8], bool) {
    let mut in_file_start = usize::MAX;
    let mut out_file_start = usize::MAX;
    let mut append = false;

    let mut i = cmd.len();
    while i > 0 {
        i -= 1;
        if cmd[i] == b'>' && out_file_start == usize::MAX {
            out_file_start = i;
            if i > 0 && cmd[i - 1] == b'>' {
                append = true;
                out_file_start = i - 1;
            }
            break;
        }
        if cmd[i] == b'<' && in_file_start == usize::MAX {
            in_file_start = i;
        }
    }

    let clean_end = if out_file_start != usize::MAX && in_file_start != usize::MAX {
        core::cmp::min(out_file_start, in_file_start)
    } else if out_file_start != usize::MAX {
        out_file_start
    } else if in_file_start != usize::MAX {
        in_file_start
    } else {
        cmd.len()
    };

    let clean_cmd = trim(&cmd[..clean_end]);
    let mut out_file: &[u8] = &[];
    let mut in_file: &[u8] = &[];

    if out_file_start != usize::MAX {
        let skip = if append { 2 } else { 1 };
        let rest = trim(&cmd[out_file_start + skip..]);
        let (name, _) = split_first(rest);
        out_file = trim(name);
    }
    if in_file_start != usize::MAX {
        // Determine where the < filename starts (after the < char)
        // If > comes before <, skip past the > section
        let scan_start = if out_file_start != usize::MAX && out_file_start < in_file_start {
            // < is after >, so the filename follows < directly
            in_file_start + 1
        } else {
            in_file_start + 1
        };
        let rest = trim(&cmd[scan_start..]);
        // Take only up to the next redirect operator if any
        let mut name_end = rest.len();
        for (i, &b) in rest.iter().enumerate() {
            if b == b'>' || b == b'<' {
                name_end = i;
                break;
            }
        }
        let (name, _) = split_first(&rest[..name_end]);
        in_file = trim(name);
    }

    (clean_cmd, in_file, out_file, append)
}

/// Split command string by pipe character '|'.
fn split_pipe(cmd: &[u8]) -> [Option<&[u8]>; MAX_CMDS_IN_PIPE] {
    let mut result: [Option<&[u8]>; MAX_CMDS_IN_PIPE] = [None; MAX_CMDS_IN_PIPE];
    let mut count = 0usize;
    let mut start = 0usize;
    for i in 0..cmd.len() {
        if cmd[i] == b'|' {
            if count < MAX_CMDS_IN_PIPE {
                result[count] = Some(trim(&cmd[start..i]));
                count += 1;
            }
            start = i + 1;
        }
    }
    if count < MAX_CMDS_IN_PIPE {
        result[count] = Some(trim(&cmd[start..]));
    }
    result
}

unsafe fn execute_single(cmd: &[u8]) -> isize {
    let (clean_cmd, in_file, out_file, append) = parse_redirects(cmd);

    let mut redir_stdin: i32 = -1;
    let mut redir_stdout: i32 = -1;

    if !in_file.is_empty() {
        let fd = syscall3(SYS_OPEN, in_file.as_ptr() as usize, in_file.len(), 0);
        if fd < 0 {
            print(b"shell: cannot open: ");
            println(in_file);
            return -1;
        }
        redir_stdin = fd as i32;
    }

    if !out_file.is_empty() {
        let flags = if append { 0x500 } else { 0x300 }; // O_CREAT|O_APPEND : O_CREAT|O_TRUNC
        let fd = syscall3(SYS_OPEN, out_file.as_ptr() as usize, out_file.len(), flags);
        if fd < 0 {
            print(b"shell: cannot open: ");
            println(out_file);
            if redir_stdin >= 0 { syscall2(SYS_CLOSE, redir_stdin as usize, 0); }
            return -1;
        }
        redir_stdout = fd as i32;
    }

    let (name, rest) = split_first(clean_cmd);
    let name = trim(name);
    let arg = trim(rest);

    if name == b"exit" || name == b"quit" {
        println(b"bye!");
        if redir_stdin >= 0 { syscall2(SYS_CLOSE, redir_stdin as usize, 0); }
        if redir_stdout >= 0 { syscall2(SYS_CLOSE, redir_stdout as usize, 0); }
        syscall1(SYS_EXIT, 0);
        loop {}
    }
    if name == b"cd" {
        if redir_stdin >= 0 { syscall2(SYS_CLOSE, redir_stdin as usize, 0); }
        if redir_stdout >= 0 { syscall2(SYS_CLOSE, redir_stdout as usize, 0); }
        builtin_cd(arg);
        return 0;
    }
    if name == b"export" {
        if redir_stdin >= 0 { syscall2(SYS_CLOSE, redir_stdin as usize, 0); }
        if redir_stdout >= 0 { syscall2(SYS_CLOSE, redir_stdout as usize, 0); }
        builtin_export(arg);
        return 0;
    }
    if name == b"help" {
        if redir_stdin >= 0 { syscall2(SYS_CLOSE, redir_stdin as usize, 0); }
        if redir_stdout >= 0 { syscall2(SYS_CLOSE, redir_stdout as usize, 0); }
        println(b"KarteOS Shell v0.5");
        println(b"Built-in: cd exit export help kill");
        println(b"Supports: | pipe, > >> < redirect, UP/DOWN history");
        println(b"Commands: ls cat echo grep sed wc head tail mkdir rm env pwd");
        return 0;
    }
    if name == b"kill" {
        if redir_stdin >= 0 { syscall2(SYS_CLOSE, redir_stdin as usize, 0); }
        if redir_stdout >= 0 { syscall2(SYS_CLOSE, redir_stdout as usize, 0); }
        let (pid_str, _) = split_first(arg);
        let pid_str = trim(pid_str);
        let mut pid = 0usize;
        for &b in pid_str {
            if b >= b'0' && b <= b'9' {
                pid = pid * 10 + (b - b'0') as usize;
            }
        }
        if pid > 0 {
            syscall2(SYS_KILL, pid, 2);
            println(b"killed");
        } else {
            println(b"kill: usage: kill PID");
        }
        return 0;
    }

    let pid = launch(name, arg, redir_stdin, redir_stdout);

    if redir_stdin >= 0 { syscall2(SYS_CLOSE, redir_stdin as usize, 0); }
    if redir_stdout >= 0 { syscall2(SYS_CLOSE, redir_stdout as usize, 0); }

    // Wait for the child process to finish before returning to the shell prompt.
    if pid > 0 {
        wait_for(pid);
    }

    pid
}

unsafe fn execute_pipeline(cmds: &[Option<&[u8]>; MAX_CMDS_IN_PIPE]) {
    let mut cmd_count = 0;
    for i in 0..MAX_CMDS_IN_PIPE {
        if cmds[i].is_some() { cmd_count += 1; } else { break; }
    }

    if cmd_count == 0 { return; }

    if cmd_count == 1 {
        let cmd = cmds[0].unwrap();
        let pid = execute_single(cmd);
        // execute_single already calls wait_for internally
        if pid < 0 {
            let (name, _) = split_first(cmd);
            print(b"command not found: ");
            println(trim(name));
        }
        return;
    }

    // Create all pipes
    let mut pipe_r_fds = [-1i32; MAX_CMDS_IN_PIPE - 1];
    let mut pipe_w_fds = [-1i32; MAX_CMDS_IN_PIPE - 1];

    for i in 0..cmd_count - 1 {
        let mut fds = [0i32; 2];
        let r = syscall1(SYS_PIPE, fds.as_mut_ptr() as usize);
        if r < 0 {
            println(b"shell: pipe failed");
            for j in 0..i {
                syscall2(SYS_CLOSE, pipe_r_fds[j] as usize, 0);
                syscall2(SYS_CLOSE, pipe_w_fds[j] as usize, 0);
            }
            return;
        }
        pipe_r_fds[i] = fds[0];
        pipe_w_fds[i] = fds[1];
    }

    let mut pids = [0isize; MAX_CMDS_IN_PIPE];

    for i in 0..cmd_count {
        let cmd = cmds[i].unwrap();
        let (clean_cmd, in_file, out_file, append) = parse_redirects(cmd);

        let redir_stdin = if !in_file.is_empty() {
            syscall3(SYS_OPEN, in_file.as_ptr() as usize, in_file.len(), 0) as i32
        } else if i > 0 {
            pipe_r_fds[i - 1]
        } else {
            -1
        };

        let redir_stdout = if !out_file.is_empty() {
            let flags = if append { 0x500 } else { 0x300 };
            syscall3(SYS_OPEN, out_file.as_ptr() as usize, out_file.len(), flags) as i32
        } else if i < cmd_count - 1 {
            pipe_w_fds[i]
        } else {
            -1
        };

        let (name, rest) = split_first(clean_cmd);
        pids[i] = launch(trim(name), trim(rest), redir_stdin, redir_stdout);

        // Close file redirect fds in shell
        if !in_file.is_empty() && redir_stdin >= 0 {
            syscall2(SYS_CLOSE, redir_stdin as usize, 0);
        }
        if !out_file.is_empty() && redir_stdout >= 0 {
            syscall2(SYS_CLOSE, redir_stdout as usize, 0);
        }
    }

    // Close all pipe fds in shell
    for i in 0..cmd_count - 1 {
        syscall2(SYS_CLOSE, pipe_r_fds[i] as usize, 0);
        syscall2(SYS_CLOSE, pipe_w_fds[i] as usize, 0);
    }

    // Wait for all children
    for i in 0..cmd_count {
        if pids[i] >= 0 { wait_for(pids[i]); }
    }

    for i in 0..cmd_count {
        if pids[i] < 0 && cmds[i].is_some() {
            let (name, _) = split_first(cmds[i].unwrap());
            print(b"command not found: ");
            println(trim(name));
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    update_cwd(b"/");

    println(b"");
    println(b"  ====================================");
    println(b"  |      KarteOS Shell v0.5          |");
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

        history_add(cmd);
        reset_hist_view();

        let has_pipe = cmd.iter().any(|&b| b == b'|');
        if has_pipe {
            let cmds = split_pipe(cmd);
            execute_pipeline(&cmds);
        } else {
            let pid = execute_single(cmd);
            // execute_single already calls wait_for internally for launched children.
            // Only handle the "command not found" case here.
            if pid < 0 {
                let (name, _) = split_first(cmd);
                print(b"command not found: ");
                println(trim(name));
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

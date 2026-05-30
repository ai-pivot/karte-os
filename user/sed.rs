// user/sed.rs — stream editor (s/pattern/replacement/[g])
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

/// Read all stdin into buf byte-by-byte, return length
fn read_stdin(buf: &mut [u8]) -> usize {
    let mut i = 0;
    while i < buf.len() {
        let mut one = [0u8; 1];
        let n = unsafe { syscall3(SYS_READ, 0, one.as_ptr() as usize, 1) };
        if n <= 0 { break; }
        buf[i] = one[0];
        i += 1;
    }
    i
}

/// Read entire file into buf, return length (0 on error)
fn read_file(path: &[u8], buf: &mut [u8]) -> usize {
    let fd = unsafe { syscall3(SYS_OPEN, path.as_ptr() as usize, path.len(), 0) };
    if fd < 0 { return 0; }
    let mut total = 0;
    while total < buf.len() {
        let n = unsafe { syscall3(SYS_READ, fd as usize, buf[total..].as_ptr() as usize, buf.len() - total) };
        if n <= 0 { break; }
        total += n as usize;
    }
    unsafe { syscall2(SYS_CLOSE, fd as usize, 0); }
    total
}

/// Trim trailing \r
fn trim_cr(s: &[u8]) -> &[u8] {
    let mut end = s.len();
    while end > 0 && s[end - 1] == b'\r' { end -= 1; }
    &s[..end]
}

/// Parse s/pattern/replacement/[g] command.
/// Returns (pattern, replacement, global) or None on parse error.
fn parse_s_cmd(cmd: &[u8]) -> Option<(&[u8], &[u8], bool)> {
    if cmd.len() < 4 || cmd[0] != b's' { return None; }
    let delim = cmd[1];

    let mut pos = 2;
    // Find end of pattern
    let p_start = pos;
    while pos < cmd.len() && cmd[pos] != delim { pos += 1; }
    if pos >= cmd.len() { return None; }
    let pattern = &cmd[p_start..pos];
    pos += 1; // skip delimiter

    // Find end of replacement
    let r_start = pos;
    while pos < cmd.len() && cmd[pos] != delim { pos += 1; }
    let replacement = &cmd[r_start..pos];

    // Check for trailing delimiter + 'g' flag
    let global = if pos < cmd.len() {
        pos += 1; // skip trailing delimiter
        pos < cmd.len() && cmd[pos] == b'g'
    } else {
        false
    };

    Some((pattern, replacement, global))
}

/// Apply substitution on a line, write result to out, return output length
fn substitute(line: &[u8], pattern: &[u8], repl: &[u8], global: bool, out: &mut [u8]) -> usize {
    if pattern.is_empty() {
        // Empty pattern: copy line as-is (no substitution)
        let len = line.len().min(out.len());
        out[..len].copy_from_slice(&line[..len]);
        return len;
    }
    let mut oi = 0;
    let mut li = 0;
    while li < line.len() && oi < out.len() {
        if li + pattern.len() <= line.len() && &line[li..li + pattern.len()] == pattern {
            // Copy replacement
            for &b in repl {
                if oi >= out.len() { break; }
                out[oi] = b;
                oi += 1;
            }
            li += pattern.len();
            if !global {
                // Copy rest of line after first match
                while li < line.len() && oi < out.len() {
                    out[oi] = line[li];
                    oi += 1;
                    li += 1;
                }
                break;
            }
        } else {
            out[oi] = line[li];
            oi += 1;
            li += 1;
        }
    }
    oi
}

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let args = get_args();
    let args = trim(&args);
    let (cmd, rest) = split_first(args);
    let cmd = trim(cmd);
    let (file, _) = split_first(trim(rest));
    let file = trim(file);

    let parsed = parse_s_cmd(cmd);
    let (pattern, repl, global) = match parsed {
        Some(v) => v,
        None => {
            println(b"Usage: sed s/pattern/replacement/[g] [file]");
            syscall1(SYS_EXIT, 1);
            loop {}
        }
    };

    let mut buf = [0u8; 4096];
    let len = if file.is_empty() {
        read_stdin(&mut buf)
    } else {
        let n = read_file(file, &mut buf);
        if n == 0 {
            print(b"sed: cannot open: ");
            println(file);
            syscall1(SYS_EXIT, 1);
            loop {}
        }
        n
    };

    let input = &buf[..len];
    let mut out = [0u8; 4096];
    let mut start = 0;
    for i in 0..input.len() {
        if input[i] == b'\n' {
            let line = trim_cr(&input[start..i]);
            let olen = substitute(line, pattern, repl, global, &mut out);
            println(&out[..olen]);
            start = i + 1;
        }
    }
    // Last line without trailing \n
    if start < input.len() {
        let line = trim_cr(&input[start..]);
        let olen = substitute(line, pattern, repl, global, &mut out);
        println(&out[..olen]);
    }

    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

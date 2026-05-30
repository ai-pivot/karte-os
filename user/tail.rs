// user/tail.rs — output last N lines
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

const MAX_LINES: usize = 64;
const MAX_LINE_LEN: usize = 128;

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

/// Parse decimal number from bytes
fn parse_u64(s: &[u8]) -> u64 {
    let mut n: u64 = 0;
    for &b in s {
        if b >= b'0' && b <= b'9' {
            n = n * 10 + (b - b'0') as u64;
        } else {
            break;
        }
    }
    n
}

/// Check if byte slice starts with a digit
fn is_number(s: &[u8]) -> bool {
    if s.is_empty() { return false; }
    s[0] >= b'0' && s[0] <= b'9'
}

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let args = get_args();
    let args = trim(&args);
    let (first, rest) = split_first(args);
    let first = trim(first);

    let n_lines: usize;
    let file: &[u8];
    if is_number(first) {
        n_lines = parse_u64(first) as usize;
        let (f, _) = split_first(trim(rest));
        file = trim(f);
    } else if first.is_empty() {
        n_lines = 10;
        file = b"";
    } else {
        n_lines = 10;
        file = first;
    }

    let mut buf = [0u8; 4096];
    let len = if file.is_empty() {
        read_stdin(&mut buf)
    } else {
        let n = read_file(file, &mut buf);
        if n == 0 {
            print(b"tail: cannot open: ");
            println(file);
            syscall1(SYS_EXIT, 1);
            loop {}
        }
        n
    };

    // Circular buffer for last N lines
    let mut lines = [[0u8; MAX_LINE_LEN]; MAX_LINES];
    let mut line_lens = [0usize; MAX_LINES];
    let mut ring_count: usize = 0; // total lines stored (capped at MAX_LINES)
    let mut ring_write: usize = 0; // next write position

    let input = &buf[..len];
    let mut start = 0;
    for i in 0..input.len() {
        if input[i] == b'\n' {
            let line = trim_cr(&input[start..i]);
            let copy_len = line.len().min(MAX_LINE_LEN);
            lines[ring_write][..copy_len].copy_from_slice(&line[..copy_len]);
            line_lens[ring_write] = copy_len;
            ring_write = (ring_write + 1) % MAX_LINES;
            if ring_count < MAX_LINES { ring_count += 1; }
            start = i + 1;
        }
    }
    // Last line without trailing \n
    if start < input.len() {
        let line = trim_cr(&input[start..]);
        let copy_len = line.len().min(MAX_LINE_LEN);
        lines[ring_write][..copy_len].copy_from_slice(&line[..copy_len]);
        line_lens[ring_write] = copy_len;
        ring_write = (ring_write + 1) % MAX_LINES;
        if ring_count < MAX_LINES { ring_count += 1; }
    }

    // Output last n_lines from circular buffer
    let want = n_lines.min(ring_count);
    let start_idx = if ring_count >= MAX_LINES {
        ring_write // oldest entry
    } else {
        0
    };
    // If we have fewer lines than MAX_LINES, offset from start to skip older ones
    let skip = if ring_count > want { ring_count - want } else { 0 };
    for j in skip..ring_count {
        let idx = (start_idx + j) % MAX_LINES;
        let llen = line_lens[idx];
        if llen > 0 {
            println(&lines[idx][..llen]);
        }
    }

    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

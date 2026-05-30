// user/head.rs — output first N lines
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

/// Check if byte slice is a positive number
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
            print(b"head: cannot open: ");
            println(file);
            syscall1(SYS_EXIT, 1);
            loop {}
        }
        n
    };

    let input = &buf[..len];
    let mut printed = 0;
    let mut start = 0;
    for i in 0..input.len() {
        if input[i] == b'\n' {
            if printed >= n_lines { break; }
            let line = trim_cr(&input[start..i]);
            println(line);
            printed += 1;
            start = i + 1;
        }
    }
    // Last line without trailing \n
    if printed < n_lines && start < input.len() {
        let line = trim_cr(&input[start..]);
        println(line);
    }

    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

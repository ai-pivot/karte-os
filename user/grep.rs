// user/grep.rs — search lines matching a pattern
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

/// Check if haystack contains needle
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() { return true; }
    if needle.len() > haystack.len() { return false; }
    for i in 0..=haystack.len() - needle.len() {
        if &haystack[i..i + needle.len()] == needle {
            return true;
        }
    }
    false
}

/// Trim trailing \r
fn trim_cr(s: &[u8]) -> &[u8] {
    let mut end = s.len();
    while end > 0 && s[end - 1] == b'\r' { end -= 1; }
    &s[..end]
}

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let args = get_args();
    let args = trim(&args);
    let (pattern, rest) = split_first(args);
    let pattern = trim(pattern);
    let (file, _) = split_first(trim(rest));
    let file = trim(file);

    if pattern.is_empty() {
        println(b"Usage: grep <pattern> [file]");
        syscall1(SYS_EXIT, 1);
        loop {}
    }

    let mut buf = [0u8; 4096];
    let len = if file.is_empty() {
        read_stdin(&mut buf)
    } else {
        let n = read_file(file, &mut buf);
        if n == 0 {
            print(b"grep: cannot open: ");
            println(file);
            syscall1(SYS_EXIT, 1);
            loop {}
        }
        n
    };

    let input = &buf[..len];
    let mut start = 0;
    for i in 0..input.len() {
        if input[i] == b'\n' {
            let line = trim_cr(&input[start..i]);
            if contains(line, pattern) {
                println(line);
            }
            start = i + 1;
        }
    }
    // Last line without trailing \n
    if start < input.len() {
        let line = trim_cr(&input[start..]);
        if contains(line, pattern) {
            println(line);
        }
    }

    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

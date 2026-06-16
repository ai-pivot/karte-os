// user/wc.rs — word count (lines, words, characters)
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

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let args = get_args();
    let (file, _) = split_first(trim(&args));
    let file = trim(file);

    let mut buf = [0u8; 4096];
    let len = if file.is_empty() {
        read_stdin(&mut buf)
    } else {
        let n = read_file(file, &mut buf);
        if n == 0 {
            print(b"wc: cannot open: ");
            println(file);
            syscall1(SYS_EXIT, 1);
            loop {}
        }
        n
    };

    let input = &buf[..len];
    let chars = input.len();

    // Count lines (number of \n characters)
    let mut lines: u64 = 0;
    for &b in input {
        if b == b'\n' { lines += 1; }
    }

    // Count words (sequences of non-whitespace)
    let mut words: u64 = 0;
    let mut in_word = false;
    for &b in input {
        let is_ws = b == b' ' || b == b'\t' || b == b'\n' || b == b'\r';
        if is_ws {
            in_word = false;
        } else if !in_word {
            in_word = true;
            words += 1;
        }
    }

    // Print: lines words chars
    print_u64(lines);
    print(b" ");
    print_u64(words);
    print(b" ");
    print_u64(chars as u64);
    println(b"");

    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

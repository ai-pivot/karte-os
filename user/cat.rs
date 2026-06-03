// user/cat.rs — show file contents
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let args = get_args();
    let args = trim(&args);
    let (path, _) = split_first(args);
    let path = trim(path);

    if path.is_empty() {
        // Read from stdin (fd 0) — supports pipe input
        let buf = [0u8; 512];
        loop {
            let n = syscall3(SYS_READ, 0, buf.as_ptr() as usize, buf.len());
            if n <= 0 { break; }
            print(&buf[..n as usize]);
        }
        syscall1(SYS_EXIT, 0);
        loop {}
    }

    let fd = syscall3(SYS_OPEN, path.as_ptr() as usize, path.len(), 0);
    if fd < 0 {
        print(b"cat: cannot open: ");
        println(path);
        syscall1(SYS_EXIT, 1);
        loop {}
    }

    let buf = [0u8; 512];
    loop {
        let n = syscall3(SYS_READ, fd as usize, buf.as_ptr() as usize, buf.len());
        if n <= 0 { break; }
        print(&buf[..n as usize]);
    }
    syscall2(SYS_CLOSE, fd as usize, 0);
    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

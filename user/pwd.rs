// user/pwd.rs — print working directory
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    match getenv(b"CWD") {
        Some(buf) => {
            let len = buf.iter().position(|&b| b == 0).unwrap_or(0);
            if len > 0 { print(&buf[..len]); }
        }
        None => print(b"/"),
    }
    println(b"");
    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

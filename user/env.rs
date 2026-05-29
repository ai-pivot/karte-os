// user/env.rs — show environment variables
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    for key in [b"PATH".as_slice(), b"USER".as_slice(), b"CWD".as_slice()].iter() {
        print(key);
        print(b"=");
        match getenv(key) {
            Some(buf) => {
                // find the null terminator or first non-printable
                let len = buf.iter().position(|&b| b == 0).unwrap_or(0);
                if len > 0 { print(&buf[..len]); }
            }
            None => print(b"(none)"),
        }
        println(b"");
    }
    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

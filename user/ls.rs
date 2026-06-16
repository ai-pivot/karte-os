// user/ls.rs — list filesystem contents
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let buf = [0u8; 4096];
    let n = syscall2(SYS_LS, buf.as_ptr() as usize, buf.len());
    if n > 0 {
        print(&buf[..n as usize]);
    }
    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

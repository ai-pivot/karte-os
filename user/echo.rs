// user/echo.rs — print arguments
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
    println(args);
    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

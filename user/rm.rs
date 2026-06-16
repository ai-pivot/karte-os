// user/rm.rs — remove files or directories
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let args = get_args();
    let (path, _) = split_first(trim(&args));
    let path = trim(path);
    if path.is_empty() {
        println(b"Usage: rm <file>");
        syscall1(SYS_EXIT, 1);
        loop {}
    }
    let r = unsafe { syscall2(SYS_UNLINK, path.as_ptr() as usize, path.len()) };
    if r < 0 {
        print(b"rm: cannot remove: ");
        println(path);
        syscall1(SYS_EXIT, 1);
        loop {}
    }
    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

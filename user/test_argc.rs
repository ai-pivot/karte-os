#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let argc: usize;
    unsafe { core::arch::asm!("ld {0}, 0(sp)", out(reg) argc); }
    let digit = b'0' + (argc as u8 % 10);
    syscall::print(b"ARGC=\0");
    syscall::print(&[digit, b'\n']);
    syscall::syscall1(syscall::SYS_EXIT, 0);
    loop { unsafe { core::arch::asm!("wfi"); } }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

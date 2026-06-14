#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    // Use Linux RISC-V syscall numbers directly
    // write(1, "OK\n", 3) = syscall 64
    unsafe {
        core::arch::asm!(
            "li a7, 64",       // Linux RISC-V write
            "li a0, 1",         // fd=1
            "la a1, 1f",        // buf=label
            "li a2, 3",         // len=3
            "ecall",
            "li a7, 94",        // Linux RISC-V exit_group
            "li a0, 0",
            "ecall",
            "1: .ascii \"OK\\n\"",
        );
    }
    loop { unsafe { core::arch::asm!("wfi"); } }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

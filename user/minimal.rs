#![no_std]
#![no_main]

#[path = "syscall.rs"]
mod syscall;

// Entry point — naked function to avoid compiler prologue (push rax).
// The very first instruction is `int 0x80` so we can confirm whether the
// CPU actually reaches user mode AND whether the syscall dispatch runs.
// If DIAG 23 appears, iretq + Ring 3 entry work. If it doesn't, the
// problem is in iretq itself or the page table walk for the entry point.
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".globl _start",
    ".type _start, @function",
    "_start:",
    "mov rax, 0x1",      // SYS_EXIT
    "mov rdi, 0x2a",     // code = 42
    "int 0x80",
    "1: jmp 1b",         // should not reach here
    ".size _start, . - _start",
);

#[cfg(target_arch = "riscv64")]
#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    use syscall::*;
    syscall1(SYS_EXIT, 42);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

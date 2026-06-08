#[cfg(target_arch = "riscv64")]
use crate::arch::sbi;

#[cfg(target_arch = "x86_64")]
fn shutdown() -> ! {
    crate::arch::platform::shutdown()
}

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::console_println!("[PANIC] {}", info);

    // Print simple backtrace via RBP chain (x86_64 only)
    #[cfg(target_arch = "x86_64")]
    {
        let mut rbp: usize;
        unsafe { core::arch::asm!("mov {}, rbp", out(reg) rbp) };
        crate::console_println!("[PANIC] Backtrace:");
        for i in 0..15 {
            if rbp == 0 || rbp < 0x100000 {
                break;
            }
            let ret_addr = unsafe { *((rbp + 8) as *const usize) };
            crate::console_println!("[PANIC]   #{}: {:#x}", i, ret_addr);
            rbp = unsafe { *(rbp as *const usize) };
        }
    }

    #[cfg(target_arch = "riscv64")]
    sbi::shutdown();

    #[cfg(target_arch = "x86_64")]
    shutdown();
}

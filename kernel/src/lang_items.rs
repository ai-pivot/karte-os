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

    #[cfg(target_arch = "riscv64")]
    sbi::shutdown();

    #[cfg(target_arch = "x86_64")]
    shutdown();
}

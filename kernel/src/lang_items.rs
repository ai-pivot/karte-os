use crate::sbi;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::console_println!("[PANIC] {}", info);
    sbi::shutdown()
}

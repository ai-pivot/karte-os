use core::panic::PanicInfo;
use crate::sbi;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::console_println!("[PANIC] {}", info);
    sbi::shutdown()
}

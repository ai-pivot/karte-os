//! Console output and system control.
//!
//! Uses direct UART MMIO for console output (works on all SBI versions)
//! and SBI standard calls for system reset and timer.

use sbi::system_reset::{ResetReason, ResetType, system_reset};

/// Write a single byte to UART0 (0x10000000) via MMIO.
///
/// This bypasses SBI entirely and writes directly to the UART hardware,
/// ensuring compatibility with all SBI versions (1.0, 2.0, etc.).
pub fn console_putchar(c: u8) {
    const UART0_BASE: usize = 0x1000_0000;
    const UART_LSR: usize = 5;
    const UART_THR: usize = 0;
    const LSR_TX_EMPTY: u8 = 0x20;

    let base = UART0_BASE;
    // Wait until TX buffer is empty
    while unsafe { core::ptr::read_volatile((base + UART_LSR) as *const u8) } & LSR_TX_EMPTY == 0 {
        core::hint::spin_loop();
    }
    unsafe {
        core::ptr::write_volatile((base + UART_THR) as *mut u8, c);
    }
}

/// Shutdown the system via SBI system reset.
pub fn shutdown() -> ! {
    let _ = system_reset(ResetType::Shutdown, ResetReason::NoReason);
    loop {}
}

/// Print a raw byte string to the console.
pub fn print(s: &str) {
    for byte in s.bytes() {
        if byte == b'\n' {
            console_putchar(b'\r');
        }
        console_putchar(byte);
    }
}

/// Formatted console output target.
pub struct Console;

impl core::fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        print(s);
        Ok(())
    }
}

#[macro_export]
macro_rules! console_print {
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            let _ = write!($crate::arch::sbi::Console, $($arg)*);
        }
    };
}

#[macro_export]
macro_rules! console_println {
    () => { $crate::console_print!("\n") };
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            let _ = writeln!($crate::arch::sbi::Console, $($arg)*);
        }
    };
}

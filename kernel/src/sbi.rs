//! SBI console output and system control wrappers.
//!
//! Uses the modern DBCN (Debug Console) extension instead of legacy SBI calls.

use sbi_rt::{NoReason, Shutdown, console_write_byte, system_reset};

/// Write a single byte to the debug console (blocking).
pub fn console_putchar(c: u8) {
    console_write_byte(c);
}

/// Shutdown the system via SBI system reset.
pub fn shutdown() -> ! {
    system_reset(Shutdown, NoReason);
    loop {}
}

/// Print a raw byte string to the debug console.
pub fn print(s: &str) {
    for byte in s.bytes() {
        console_write_byte(byte);
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
            let _ = write!($crate::sbi::Console, $($arg)*);
        }
    };
}

#[macro_export]
macro_rules! console_println {
    () => { $crate::console_print!("\n") };
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            let _ = writeln!($crate::sbi::Console, $($arg)*);
        }
    };
}

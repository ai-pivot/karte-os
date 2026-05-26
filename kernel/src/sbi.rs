/// SBI console output and system control wrappers
use sbi_rt::{self, system_reset, Shutdown, NoReason};

/// Print to SBI console (low-level, single character)
pub fn console_putchar(c: u8) {
    sbi_rt::legacy::console_putchar(c as usize);
}

/// Shutdown the system via SBI
pub fn shutdown() -> ! {
    system_reset(Shutdown, NoReason);
    loop {}
}

/// Print a string to SBI console
pub fn print(s: &str) {
    for byte in s.bytes() {
        console_putchar(byte);
    }
}

/// Print formatted string to SBI console
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

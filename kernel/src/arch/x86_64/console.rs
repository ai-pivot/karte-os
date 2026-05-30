//! Console output for x86_64 via COM1 serial port.
//!
//! Provides the same `console_print!` and `console_println!` macros as the
//! RISC-V version, but writes to COM1 (I/O port 0x3F8) instead of MMIO UART.

/// Write a single byte to COM1 (console output).
pub fn console_putchar(c: u8) {
    crate::arch::platform::console_putchar(c);
}

/// Print a raw string to the console.
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

#[cfg(target_arch = "x86_64")]
#[macro_export]
macro_rules! console_print {
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            let _ = write!($crate::arch::console::Console, $($arg)*);
        }
    };
}

#[cfg(target_arch = "x86_64")]
#[macro_export]
macro_rules! console_println {
    () => { $crate::console_print!("\n") };
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            let _ = writeln!($crate::arch::console::Console, $($arg)*);
        }
    };
}

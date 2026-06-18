//! Console output for x86_64 via COM1 serial port.
//!
//! Provides the same `console_print!` and `console_println!` macros as the
//! RISC-V version, but writes to COM1 (I/O port 0x3F8) instead of MMIO UART.

/// Write a single byte to COM1 (console output).
pub fn console_putchar(c: u8) {
    crate::arch::platform::console_putchar(c);
}

/// Print a raw string to the console.
///
/// In raw mode (TUI programs own the VGA screen), kernel output goes to
/// UART and the kernel log buffer only, NOT VGA. This prevents kernel
/// log messages from corrupting TUI layouts.
pub fn print(s: &str) {
    let raw_mode = crate::driver::vga::is_raw_mode();
    for byte in s.bytes() {
        // Always write to kernel log buffer (for dmesg)
        crate::kernel_log::log_write_byte(byte);
        // UART always gets output (for debugging via serial)
        if byte == b'\n' {
            crate::arch::uart::putchar(b'\r');
            if !raw_mode {
                crate::driver::vga::putchar(b'\r');
            }
        }
        crate::arch::uart::putchar(byte);
        // VGA only in non-raw mode (prevents TUI corruption)
        if !raw_mode {
            crate::driver::vga::putchar(byte);
        }
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

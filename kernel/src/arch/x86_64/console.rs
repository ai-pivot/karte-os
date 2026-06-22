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

    // Process all bytes: kernel log buffer + VGA (if not raw mode)
    // Collect UART bytes for batch output
    let bytes = s.as_bytes();
    let mut uart_buf: [u8; 256] = [0; 256];
    let mut uart_len = 0usize;

    for &byte in bytes {
        // Always write to kernel log buffer (for dmesg)
        crate::kernel_log::log_write_byte(byte);
        // Handle \n → \r\n for UART
        if byte == b'\n' {
            if uart_len < 255 {
                uart_buf[uart_len] = b'\r';
                uart_len += 1;
            }
            if !raw_mode {
                crate::driver::vga::putchar(b'\r');
            }
        }
        if uart_len < 256 {
            uart_buf[uart_len] = byte;
            uart_len += 1;
        }
        // VGA only in non-raw mode (prevents TUI corruption)
        if !raw_mode {
            crate::driver::vga::putchar(byte);
        }
        // Framebuffer console (UEFI GOP fallback when VGA not available)
        crate::arch::fb_console::putchar(byte);
    }

    // Batch UART output (single write_batch call instead of per-byte putchar)
    if uart_len > 0 {
        let mut uart = crate::arch::uart::ComPort::new(0x3F8);
        uart.write_batch(&uart_buf[..uart_len]);
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

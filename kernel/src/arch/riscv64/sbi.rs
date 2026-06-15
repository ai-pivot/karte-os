//! Console output and system control.
//!
//! Uses direct UART MMIO for console output (works on all SBI versions)
//! and SBI standard calls for system reset and timer.

use sbi::system_reset::{ResetReason, ResetType, system_reset};

/// Write a single byte to UART0 (0x10000000) via MMIO.
///
/// UART is identity-mapped in ALL page tables (via copy_kernel_mappings),
/// so no SATP switching is needed. The PF handler's MMIO exclusion ensures
/// lazy allocation never overwrites the UART PTE.
pub fn console_putchar(c: u8) {
    const UART0_BASE: usize = 0x1000_0000;
    const UART_LSR: usize = 5;
    const UART_THR: usize = 0;
    const LSR_TX_EMPTY: u8 = 0x20;

    unsafe {
        while core::ptr::read_volatile((UART0_BASE + UART_LSR) as *const u8) & LSR_TX_EMPTY == 0 {
            core::hint::spin_loop();
        }
        core::ptr::write_volatile((UART0_BASE + UART_THR) as *mut u8, c);
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
        // Write to log buffer first (lock-free, never fails)
        crate::kernel_log::log_write_byte(byte);
        // Then write to UART hardware
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

#[cfg(target_arch = "riscv64")]
#[macro_export]
macro_rules! console_print {
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            let _ = write!($crate::arch::sbi::Console, $($arg)*);
        }
    };
}

#[cfg(target_arch = "riscv64")]
#[macro_export]
macro_rules! console_println {
    () => { $crate::console_print!("\n") };
    ($($arg:tt)*) => {
        {
            use core::fmt::Write
            ;
            let _ = writeln!($crate::arch::sbi::Console, $($arg)*);
        }
    };
}

/// Trace output - uses console_putchar (with SATP switching) for reliability.
#[allow(dead_code)]
pub fn raw_trace(msg: &str) {
    for &b in msg.as_bytes() {
        console_putchar(b);
    }
}

/// Raw trace with hex value
#[allow(dead_code)]
pub fn raw_trace_hex(prefix: &str, val: usize) {
    raw_trace(prefix);
    let mut buf = [0u8; 19];
    buf[0] = b'0';
    buf[1] = b'x';
    let mut idx = 2;
    for shift in (0..64).step_by(4).rev() {
        let nibble = (val >> shift) & 0xf;
        if nibble != 0 || idx > 2 || shift == 0 {
            buf[idx] = if nibble < 10 {
                b'0' + nibble as u8
            } else {
                b'a' + (nibble - 10) as u8
            };
            idx += 1;
        }
    }
    let s = core::str::from_utf8(&buf[..idx]).unwrap_or("?");
    raw_trace(s);
    raw_trace("\n");
}

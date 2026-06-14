//! Console output and system control.
//!
//! Uses direct UART MMIO for console output (works on all SBI versions)
//! and SBI standard calls for system reset and timer.

use sbi::system_reset::{ResetReason, ResetType, system_reset};

/// Write a single byte to UART0 (0x10000000) via MMIO.
///
/// Switches to kernel page table (SATP) before accessing UART MMIO,
/// then restores the original SATP. This ensures UART is always
/// accessible via the kernel's identity mapping, regardless of
/// the current process's page table state.
pub fn console_putchar(c: u8) {
    const UART0_BASE: usize = 0x1000_0000;
    const UART_LSR: usize = 5;
    const UART_THR: usize = 0;
    const LSR_TX_EMPTY: u8 = 0x20;

    unsafe {
        // Save current satp
        let saved_satp: usize;
        core::arch::asm!("csrr {}, satp", out(reg) saved_satp);

        // Switch to kernel page table for reliable UART MMIO access
        let kernel_satp =
            crate::mm::vmm::KERNEL_SATP.load(core::sync::atomic::Ordering::Relaxed);
        let need_switch = kernel_satp != 0 && kernel_satp != saved_satp;
        if need_switch {
            core::arch::asm!(
                "csrw satp, {ks}",
                "sfence.vma",
                ks = in(reg) kernel_satp,
            );
        }

        // Write to UART
        while core::ptr::read_volatile((UART0_BASE + UART_LSR) as *const u8) & LSR_TX_EMPTY == 0 {
            core::hint::spin_loop();
        }
        core::ptr::write_volatile((UART0_BASE + UART_THR) as *mut u8, c);

        // Restore original satp
        if need_switch {
            core::arch::asm!(
                "csrw satp, {ss}",
                "sfence.vma",
                ss = in(reg) saved_satp,
            );
        }
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

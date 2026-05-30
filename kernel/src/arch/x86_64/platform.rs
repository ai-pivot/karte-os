//! x86_64 platform abstraction — replaces RISC-V SBI and platform-specific functions.
//!
//! This module provides the same public interface as the RISC-V version's
//! combination of SBI calls, CSR manipulation, and MMIO, but implemented
//! using x86_64 instructions via the `x86_64` crate.

/// Disable interrupts (cli instruction).
pub fn irq_disable() {
    x86_64::instructions::interrupts::disable();
}

/// Enable interrupts (sti instruction).
pub fn irq_enable() {
    x86_64::instructions::interrupts::enable();
}

/// Disable interrupts and return the previous interrupt state.
/// Returns the RFLAGS register value with the IF bit preserved.
pub fn irq_save() -> usize {
    let flags: u64;
    unsafe {
        core::arch::asm!(
            "pushf",      // push RFLAGS
            "pop {}",      // pop into output register
            "cli",         // disable interrupts
            out(reg) flags,
            options(nomem, preserves_flags),
        );
    }
    flags as usize
}

/// Restore interrupt state from a previously saved RFLAGS value.
pub fn irq_restore(flags: usize) {
    if flags & 0x200 != 0 {
        // IF bit was set — re-enable interrupts
        irq_enable();
    }
    // If IF was clear, interrupts stay disabled (cli was already called)
}

/// Shut down the system.
/// Uses QEMU's `isa-debug-exit` device at I/O port 0x501.
/// Exit code is `(value << 1) | 1`, so writing 0x31 gives exit code 99.
pub fn shutdown() -> ! {
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(0x501);
        port.write(0x31u8);
    }
    // If isa-debug-exit is not available, fall through to infinite halt
    loop {
        x86_64::instructions::hlt();
    }
}

/// Write a single byte to the console.
///
/// On x86_64, output goes to both COM1 serial port and VGA text buffer.
pub fn console_putchar(c: u8) {
    // COM1 serial output
    unsafe {
        let mut lsr: x86_64::instructions::port::Port<u8> =
            x86_64::instructions::port::Port::new(0x3FD);

        // Wait until THR is empty (bit 5 of LSR)
        while lsr.read() & 0x20 == 0 {}

        let mut data: x86_64::instructions::port::Port<u8> =
            x86_64::instructions::port::Port::new(0x3F8);
        data.write(c);
    }

    // VGA text mode output (if initialized)
    crate::driver::vga::putchar(c);
}

/// Execute the HLT instruction — halt until the next interrupt.
pub fn wait_for_interrupt() {
    x86_64::instructions::hlt();
}

/// Read the current CPU (hart) ID.
/// On x86_64 this is the LAPIC ID; for single-core stub, returns 0.
pub fn current_hart() -> usize {
    crate::arch::smp::current_hart()
}

/// Invalidate the entire TLB.
pub fn flush_tlb() {
    x86_64::instructions::tlb::flush_all();
}

/// Invalidate a single TLB entry for the given virtual address.
pub fn flush_tlb_addr(addr: usize) {
    let vaddr = x86_64::VirtAddr::new(addr as u64);
    x86_64::instructions::tlb::flush(vaddr);
}

/// Activate a page table by writing to CR3.
pub fn activate_page_table(root_paddr: usize) {
    crate::arch::trap::activate_page_table(root_paddr);
}

/// Read the current page table root from CR3.
pub fn read_page_table_root() -> usize {
    crate::arch::trap::read_page_table_root()
}

/// Print a string to the console via COM1.
pub fn print(s: &str) {
    crate::arch::console::print(s);
}

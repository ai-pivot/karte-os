//! RISC-V platform abstraction — SBI and CSR helpers.
//!
//! Provides the same interface as x86_64's platform module but
//! implemented via SBI calls and CSR manipulation.

/// Disable interrupts (clear SIE in sstatus).
pub fn irq_disable() {
    unsafe { core::arch::asm!("csrci sstatus, 2") };
}

/// Enable interrupts (set SIE in sstatus).
pub fn irq_enable() {
    unsafe { core::arch::asm!("csrsi sstatus, 2") };
}

/// Disable interrupts and return previous state.
pub fn irq_save() -> usize {
    let sstatus: usize;
    unsafe {
        core::arch::asm!(
            "csrr {}, sstatus",
            "csrci sstatus, 2",
            out(reg) sstatus,
        );
    }
    sstatus
}

/// Restore interrupt state.
pub fn irq_restore(_flags: usize) {
    // Simplified: just enable interrupts
    irq_enable();
}

/// Shutdown the system via SBI.
pub fn shutdown() -> ! {
    crate::arch::sbi::shutdown()
}

/// Write a single byte to the console (UART MMIO).
pub fn console_putchar(c: u8) {
    crate::arch::sbi::console_putchar(c)
}

/// Print a string to the console.
pub fn print(s: &str) {
    crate::arch::sbi::print(s)
}

/// Wait for interrupt (wfi instruction).
pub fn wait_for_interrupt() {
    unsafe { core::arch::asm!("wfi") };
}

/// Read the current hart ID.
pub fn current_hart() -> usize {
    crate::arch::smp::current_hart()
}

/// Flush the TLB.
pub fn flush_tlb() {
    unsafe { core::arch::asm!("sfence.vma") };
}

/// Flush TLB for a specific address.
pub fn flush_tlb_addr(_addr: usize) {
    // sfence.vma with rs1 = addr, rs2 = x0
    unsafe { core::arch::asm!("sfence.vma") };
}

/// Activate page table (write satp + sfence.vma).
pub fn activate_page_table(root_paddr: usize) {
    let ppn = root_paddr >> 12;
    let satp_val = (8usize << 60) | ppn; // Sv39 mode
    unsafe {
        core::arch::asm!("csrw satp, {}", in(reg) satp_val);
        core::arch::asm!("sfence.vma");
    }
}

/// Read current page table root.
pub fn read_page_table_root() -> usize {
    let satp: usize;
    unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp) };
    (satp & ((1usize << 44) - 1)) << 12
}

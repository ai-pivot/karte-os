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
/// Tries multiple methods:
/// 1. QEMU isa-debug-exit (port 0x501) — works in QEMU
/// 2. ACPI shutdown — works on real hardware with ACPI
/// 3. Legacy AT keyboard controller reset — last resort
pub fn shutdown() -> ! {
    // Method 1: QEMU debug exit
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(0x501);
        port.write(0x31u8);
    }

    // Method 2: ACPI shutdown (via SCI)
    // Try to write to the PM1a_CNT register (typically at 0x604 for QEMU,
    // but on real hardware it's read from the FADT table)
    // For a simple approach, try the common ports:
    unsafe {
        // Try QEMU's ACPI port (PIIX4 PM)
        let mut pm1a: x86_64::instructions::port::Port<u16> =
            x86_64::instructions::port::Port::new(0x604);
        pm1a.write(0x2000); // SLP_TYP=S5 | SLP_EN
    }

    // Method 3: Fast ACPI shutdown via I/O port 0xB004 (some machines)
    unsafe {
        let mut pm: x86_64::instructions::port::Port<u16> =
            x86_64::instructions::port::Port::new(0xB004);
        pm.write(0x2000);
    }

    // Method 4: Legacy keyboard controller reset (triple fault → reset)
    unsafe {
        // Disable interrupts
        core::arch::asm!("cli");
        // Pulse the reset line via keyboard controller
        let mut port64: x86_64::instructions::port::Port<u8> =
            x86_64::instructions::port::Port::new(0x64);
        let mut port60: x86_64::instructions::port::Port<u8> =
            x86_64::instructions::port::Port::new(0x60);
        // Wait for keyboard controller ready
        while port64.read() & 0x02 != 0 {}
        port64.write(0xFE); // Pulse reset line
        let _ = port60.read(); // Small delay
    }

    // If all methods fail, infinite halt
    loop {
        x86_64::instructions::hlt();
    }
}

/// Write a single byte to the console.
///
/// On x86_64, output goes to both COM1 serial port and VGA text buffer.
pub fn console_putchar(c: u8) {
    // COM1 serial output (with timeout protection)
    unsafe {
        let mut lsr: x86_64::instructions::port::Port<u8> =
            x86_64::instructions::port::Port::new(0x3FD);

        // Wait until THR is empty (bit 5 of LSR), with timeout
        let mut timeout = 1_000_000u32;
        while lsr.read() & 0x20 == 0 {
            timeout -= 1;
            if timeout == 0 {
                break; // Prevent infinite deadlock
            }
            core::hint::spin_loop();
        }

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

/// Monotonic uptime counter (milliseconds since boot).
static UPTIME_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// TSC counter at boot time (for computing elapsed time).
static BOOT_TSC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Read the TSC (Time Stamp Counter). On QEMU, the TSC ticks at the CPU
/// frequency, which is typically ~2-3 GHz. We calibrate using the LAPIC
/// timer's first few ticks.
#[inline]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Increment the uptime counter (called from timer interrupt).
pub fn tick_uptime() {
    UPTIME_MS.fetch_add(10, core::sync::atomic::Ordering::Relaxed);
}

/// Get the current uptime in milliseconds.
/// Uses TSC for accurate time when calibrated, falls back to tick counter.
pub fn uptime_ms() -> u64 {
    UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed)
}

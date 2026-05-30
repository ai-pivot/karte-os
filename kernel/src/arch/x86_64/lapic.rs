//! Local APIC (Advanced Programmable Interrupt Controller) driver.
//!
//! On x86_64, the LAPIC replaces the legacy 8259 PIC. Each CPU core has
//! its own LAPIC at physical address 0xFEE00000 (default).
//!
//! Key functions:
//! - Timer interrupts (replaces RISC-V's SBI timer)
//! - EOI (End of Interrupt) signaling
//! - Interrupt masking/unmasking
//!
//! We access the LAPIC via MMIO (memory-mapped I/O), using volatile
//! reads/writes through the identity-mapped virtual address.

use x86_64::VirtAddr;
use x86_64::registers::model_specific::Msr;

/// IA32_APIC_BASE MSR address.
const IA32_APIC_BASE_MSR: u32 = 0x1B;

/// LAPIC default physical base address.
const LAPIC_DEFAULT_PHYS: u64 = 0xFEE00000;

/// LAPIC register offsets.
mod reg {
    pub const ID: u64 = 0x020;
    pub const VERSION: u64 = 0x030;
    pub const TASK_PRIORITY: u64 = 0x080;
    pub const ARBITRATION_PRIORITY: u64 = 0x090;
    pub const PROCESSOR_PRIORITY: u64 = 0x0A0;
    pub const EOI: u64 = 0x0B0;
    pub const LOGICAL_DESTINATION: u64 = 0x0D0;
    pub const DESTINATION_FORMAT: u64 = 0x0E0;
    pub const SPURIOUS_VECTOR: u64 = 0x0F0;
    pub const ERROR_STATUS: u64 = 0x280;
    pub const LVT_TIMER: u64 = 0x320;
    pub const LVT_THERMAL: u64 = 0x330;
    pub const LVT_PERF: u64 = 0x340;
    pub const LVT_LINT0: u64 = 0x350;
    pub const LVT_LINT1: u64 = 0x360;
    pub const LVT_ERROR: u64 = 0x370;
    pub const TIMER_INITIAL_COUNT: u64 = 0x380;
    pub const TIMER_CURRENT_COUNT: u64 = 0x390;
    pub const TIMER_DIVIDE: u64 = 0x3E0;
}

/// Timer mode bits in LVT Timer register.
const TIMER_MODE_PERIODIC: u32 = 0x20000;

/// LAPIC driver.
pub struct Lapic {
    base: VirtAddr,
}

impl Lapic {
    /// Create a new LAPIC instance.
    /// Reads the base address from IA32_APIC_BASE MSR.
    pub fn new() -> Self {
        let msr = unsafe { Msr::new(IA32_APIC_BASE_MSR) };
        let base_phys = (unsafe { msr.read() } & 0xFFFF_F000) as u64;
        // Assume identity mapping (phys = virt for low addresses)
        let base = VirtAddr::new(base_phys);
        Self { base }
    }

    /// Initialize the LAPIC: enable it and set up the spurious interrupt vector.
    pub fn init(&self) {
        // Enable LAPIC via the spurious interrupt vector register.
        // Bit 8 = APIC software enable, vector = 0xFF (spurious)
        self.write(reg::SPURIOUS_VECTOR, 0x1FF);
    }

    /// Set up the LAPIC timer for periodic interrupts.
    ///
    /// - `divide`: divide configuration (e.g., 0x03 = divide by 16)
    /// - `initial_count`: initial count value (determines period)
    /// - `vector`: IDT vector number for the timer interrupt
    pub fn setup_timer(&self, divide: u32, initial_count: u32, vector: u8) {
        // Set divide value
        self.write(reg::TIMER_DIVIDE, divide);
        // Set initial count
        self.write(reg::TIMER_INITIAL_COUNT, initial_count);
        // Set LVT timer entry: periodic mode + vector
        self.write(reg::LVT_TIMER, TIMER_MODE_PERIODIC | vector as u32);
    }

    /// Signal End of Interrupt (EOI).
    /// Must be called at the end of every interrupt handler.
    pub fn eoi(&self) {
        self.write(reg::EOI, 0);
    }

    /// Get the LAPIC ID (unique per core).
    pub fn id(&self) -> u32 {
        self.read(reg::ID) >> 24
    }

    /// Read a LAPIC register.
    fn read(&self, offset: u64) -> u32 {
        unsafe {
            let addr = (self.base.as_u64() + offset) as *const u32;
            core::ptr::read_volatile(addr)
        }
    }

    /// Write a LAPIC register.
    fn write(&self, offset: u64, value: u32) {
        unsafe {
            let addr = (self.base.as_u64() + offset) as *mut u32;
            core::ptr::write_volatile(addr, value);
        }
    }
}

// ─── Global LAPIC instance ──────────────────────────────────

/// Direct LAPIC access functions that don't depend on the global Once.
/// Each CPU's LAPIC is at the same physical address but has independent registers.

/// Read LAPIC register directly from the MMIO base.
unsafe fn lapic_read(offset: u64) -> u32 {
    let msr = Msr::new(IA32_APIC_BASE_MSR);
    let base_phys = (msr.read() & 0xFFFF_F000) as u64;
    let addr = (base_phys + offset) as *const u32;
    core::ptr::read_volatile(addr)
}

/// Write LAPIC register directly.
unsafe fn lapic_write(offset: u64, value: u32) {
    let msr = Msr::new(IA32_APIC_BASE_MSR);
    let base_phys = (msr.read() & 0xFFFF_F000) as u64;
    let addr = (base_phys + offset) as *mut u32;
    core::ptr::write_volatile(addr, value);
}

/// Initialize the LAPIC for the current CPU.
/// Can be called multiple times (once per CPU in SMP).
pub fn init() {
    unsafe {
        // Enable LAPIC via the spurious interrupt vector register.
        lapic_write(reg::SPURIOUS_VECTOR, 0x1FF);
    }
}

/// Signal End of Interrupt to the current CPU's LAPIC.
pub fn local_eoi() {
    unsafe {
        lapic_write(reg::EOI, 0);
    }
}

/// Enable the LAPIC timer for periodic interrupts.
pub fn enable_timer() {
    unsafe {
        // Divide by 16
        lapic_write(reg::TIMER_DIVIDE, 0x03);
        // Initial count: ~100MHz / 16 / 62500 ≈ 100Hz → ~10ms per tick
        lapic_write(reg::TIMER_INITIAL_COUNT, 62500);
        // LVT timer: periodic mode + vector
        lapic_write(
            reg::LVT_TIMER,
            TIMER_MODE_PERIODIC | super::idt::TIMER_VECTOR as u32,
        );
    }
}

/// Set up the next timer tick (not needed for periodic mode).
pub fn set_next_timer() {
    // In periodic mode, the timer automatically reloads.
}

/// Get the current CPU's LAPIC ID.
pub fn lapic_id() -> u32 {
    unsafe { lapic_read(reg::ID) >> 24 }
}

/// Send an INIT IPI to a target LAPIC (for SMP startup).
pub fn send_init_ipi(target_apic_id: u32) {
    unsafe {
        lapic_write(0x310, target_apic_id << 24);
        lapic_write(0x300, 0x00004500);
    }
}

/// Send a STARTUP IPI to a target LAPIC (for SMP startup).
pub fn send_startup_ipi(target_apic_id: u32, vector: u8) {
    unsafe {
        lapic_write(0x310, target_apic_id << 24);
        lapic_write(0x300, 0x00004600 | vector as u32);
    }
}

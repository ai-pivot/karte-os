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

use x86_64::registers::model_specific::Msr;
use x86_64::VirtAddr;

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
    pub fn init(&mut self) {
        // Enable LAPIC via the spurious interrupt vector register.
        // Bit 8 = APIC software enable, vector = 0xFF (spurious)
        self.write(reg::SPURIOUS_VECTOR, 0x1FF);
    }

    /// Set up the LAPIC timer for periodic interrupts.
    ///
    /// - `divide`: divide configuration (e.g., 0x03 = divide by 16)
    /// - `initial_count`: initial count value (determines period)
    /// - `vector`: IDT vector number for the timer interrupt
    pub fn setup_timer(&mut self, divide: u32, initial_count: u32, vector: u8) {
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

use spin::Once;

static LAPIC: Once<Lapic> = Once::new();

/// Initialize the LAPIC for the current CPU.
pub fn init() {
    LAPIC.call_once(|| {
        let mut lapic = Lapic::new();
        lapic.init();
        lapic
    });
}

/// Signal End of Interrupt to the LAPIC.
pub fn local_eoi() {
    if let Some(lapic) = LAPIC.get() {
        lapic.eoi();
    }
}

/// Enable the LAPIC timer for periodic interrupts.
/// Uses divide-by-16 and a reasonable initial count for ~10ms ticks.
pub fn enable_timer() {
    if let Some(lapic) = LAPIC.get() {
        // Divide by 16
        // Initial count: assumes ~100MHz APIC bus frequency
        // 100MHz / 16 / 10000 = ~625 Hz → ~1.6ms per tick
        // For 10ms: 100MHz / 16 * 0.01 = 62500
        lapic.setup_timer(0x03, 62500, super::idt::TIMER_VECTOR);
    }
}

/// Set up the next timer tick (not needed for periodic mode,
/// but provided for API compatibility with RISC-V).
pub fn set_next_timer() {
    // In periodic mode, the timer automatically reloads.
    // No action needed.
}

/// Get the current CPU's LAPIC ID.
pub fn lapic_id() -> u32 {
    match LAPIC.get() {
        Some(lapic) => lapic.id(),
        None => 0,
    }
}

/// Send an INIT IPI to a target LAPIC (for SMP startup).
pub fn send_init_ipi(target_apic_id: u32) {
    // ICRL (low 32 bits of Interrupt Command Register) at offset 0x300
    // ICRH (high 32 bits) at offset 0x310
    if let Some(lapic) = LAPIC.get() {
        lapic.write(0x310, target_apic_id << 24); // Destination field
        lapic.write(0x300, 0x00004500); // INIT IPI, assert, all logical
    }
}

/// Send a STARTUP IPI to a target LAPIC (for SMP startup).
/// `vector` is the page number where the AP startup code lives (real mode).
pub fn send_startup_ipi(target_apic_id: u32, vector: u8) {
    if let Some(lapic) = LAPIC.get() {
        lapic.write(0x310, target_apic_id << 24);
        lapic.write(0x300, 0x00004600 | vector as u32); // STARTUP IPI
    }
}

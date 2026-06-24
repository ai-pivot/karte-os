//! I/O APIC (Advanced Programmable Interrupt Controller) driver.
//!
//! The IOAPIC routes external hardware interrupts (IRQs) from devices like
//! the keyboard, UART, and disk controllers to the Local APIC of a specific
//! CPU core. Without IOAPIC configuration, external interrupts never arrive.
//!
//! On QEMU `pc` machine, the IOAPIC is at physical address 0xFEC00000.

use x86_64::VirtAddr;

// ── IOAPIC address constants ─────────────────────────────────────────────

/// Fallback physical base address (QEMU pc machine default).
const IOAPIC_BASE_PHYS: u64 = 0xFEC00000;

/// IOAPIC physical base addresses to probe at boot.
///
/// The default QEMU address is 0xFEC00000, but real chipsets may place
/// the IOAPIC elsewhere (0xFEC01000, 0xFEC80000, etc.).  We probe each
/// candidate by reading the IOAPIC version register; a valid response
/// (version > 0 && version < 0xFF) confirms the device.
const IOAPIC_CANDIDATES: &[u64] = &[
    0xFEC00000, // QEMU / common Intel / AMD
    0xFEC01000, // alternate
    0xFEC80000, // some Intel PCH variants
];

/// Returns the first IOAPIC physical base address that responds to a
/// version-register read, or falls back to 0xFEC00000.
fn probe_ioapic_address() -> u64 {
    for &addr in IOAPIC_CANDIDATES {
        // The IOAPIC ID register is at offset 0x00; version is at 0x01.
        // Write 0x01 to IOREGSEL (offset 0), read IOWIN (offset 0x10).
        let base = addr;
        // Verify the region is identity-mapped before touching it
        unsafe {
            core::ptr::write_volatile(base as *mut u32, 0x01u32); // select version reg
            let ver = core::ptr::read_volatile((base + 0x10) as *const u32);
            let version = ver & 0xFF;
            if version > 0 && version < 0xFF {
                crate::console_println!("[ioapic] found at {:#x} version={}", base, version);
                return base;
            }
        }
    }
    // Fallback to default
    crate::console_println!("[ioapic] using default {:#x}", IOAPIC_BASE_PHYS);
    IOAPIC_BASE_PHYS
}

/// IOAPIC base address discovered at init time.
/// Lazily initialised on first access.
static IOAPIC_BASE: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// IOAPIC register offsets (MMIO).
const REG_IOREGSEL: u64 = 0x00; // I/O Register Select (write-only)
const REG_IOWIN: u64 = 0x10; // I/O Window (read/write, 32-bit)

/// IOAPIC register indices.
const REG_ID: u32 = 0x00; // IOAPIC ID
const REG_VER: u32 = 0x01; // IOAPIC Version
const REG_ARB: u32 = 0x02; // Arbitration ID

/// Redirection entry base index. Each entry is 2 registers (low, high).
/// Entry N = IOREDTBL + 2*N (low) and IOREDTBL + 2*N + 1 (high).
const IOREDTBL: u32 = 0x10;

/// Redirection entry bits (low 32-bit register).
const ENTRY_MASKED: u32 = 1 << 16; // Interrupt mask (1 = masked)
const ENTRY_LEVEL: u32 = 1 << 15; // Trigger mode (1 = level, 0 = edge)
const ENTRY_LOWPrio: u32 = 1 << 13; // Delivery mode = Lowest Priority
const ENTRY_SMI: u32 = 0b010 << 8; // SMI
const ENTRY_NMI: u32 = 0b100 << 8; // NMI
const ENTRY_INIT: u32 = 0b101 << 8; // INIT
const ENTRY_EXTINT: u32 = 0b111 << 8; // ExtINT

/// Delivery mode: Fixed (normal interrupt).
const ENTRY_FIXED: u32 = 0b000 << 8;

/// Delivery status (read-only): 1 = send pending.
const ENTRY_SEND_PENDING: u32 = 1 << 12;

/// Polarity: 0 = active high, 1 = active low.
const ENTRY_ACTIVE_LOW: u32 = 1 << 13;

/// Trigger mode: 0 = edge, 1 = level.
const ENTRY_LEVEL_TRIGGERED: u32 = 1 << 15;

/// The IOAPIC device.
pub struct IoApic {
    base: VirtAddr,
}

impl IoApic {
    /// Create a new IOAPIC instance at the default physical address.
    /// Assumes identity mapping is set up for the MMIO range.
    pub fn new() -> Self {
        let addr = if IOAPIC_BASE.load(core::sync::atomic::Ordering::Relaxed) == 0 {
            IOAPIC_BASE_PHYS
        } else {
            IOAPIC_BASE.load(core::sync::atomic::Ordering::Relaxed)
        };
        Self {
            base: VirtAddr::new(addr),
        }
    }

    /// Read a 32-bit IOAPIC register.
    fn read(&self, reg: u32) -> u32 {
        unsafe {
            // Write register index to IOREGSEL
            core::ptr::write_volatile(self.base.as_u64() as *mut u32, reg);
            // Read value from IOWIN
            core::ptr::read_volatile((self.base.as_u64() + REG_IOWIN) as *const u32)
        }
    }

    /// Write a 32-bit IOAPIC register.
    fn write(&self, reg: u32, value: u32) {
        unsafe {
            // Write register index to IOREGSEL
            core::ptr::write_volatile(self.base.as_u64() as *mut u32, reg);
            // Write value to IOWIN
            core::ptr::write_volatile((self.base.as_u64() + REG_IOWIN) as *mut u32, value);
        }
    }

    /// Get the IOAPIC ID.
    pub fn id(&self) -> u32 {
        self.read(REG_ID) >> 24
    }

    /// Get the IOAPIC version.
    /// Bits [7:0] = version, Bits [23:16] = max redirection entry count.
    pub fn version(&self) -> u32 {
        self.read(REG_VER)
    }

    /// Get the number of redirection entries (IRQ pins).
    pub fn max_entries(&self) -> u32 {
        (self.read(REG_VER) >> 16) & 0xFF
    }

    /// Read a full 64-bit redirection entry for the given IRQ pin.
    fn read_redir_entry(&self, irq: u8) -> u64 {
        let low_reg = IOREDTBL + 2 * irq as u32;
        let high_reg = low_reg + 1;
        let low = self.read(low_reg) as u64;
        let high = self.read(high_reg) as u64;
        (high << 32) | low
    }

    /// Write a full 64-bit redirection entry for the given IRQ pin.
    fn write_redir_entry(&self, irq: u8, entry: u64) {
        let low_reg = IOREDTBL + 2 * irq as u32;
        let high_reg = low_reg + 1;
        self.write(low_reg, entry as u32);
        self.write(high_reg, (entry >> 32) as u32);
    }

    /// Configure a redirection entry for an IRQ pin.
    ///
    /// - `irq`: IRQ pin number (0-23)
    /// - `vector`: IDT vector number (32-255)
    /// - `masked`: whether to mask (disable) the interrupt
    /// - `level_triggered`: trigger mode (true = level, false = edge)
    /// - `active_low`: polarity (true = active low, false = active high)
    pub fn set_irq(
        &self,
        irq: u8,
        vector: u8,
        masked: bool,
        level_triggered: bool,
        active_low: bool,
    ) {
        let mut entry: u64 = vector as u64; // Vector [7:0]
        entry |= ENTRY_FIXED as u64; // Delivery mode = Fixed

        if level_triggered {
            entry |= ENTRY_LEVEL_TRIGGERED as u64;
        }
        if active_low {
            entry |= ENTRY_ACTIVE_LOW as u64;
        }
        if masked {
            entry |= ENTRY_MASKED as u64;
        }
        // Destination: APIC ID 0 (BSP) in bits [63:56]
        // For physical delivery mode, destination is APIC ID.
        entry |= 0u64 << 56;

        self.write_redir_entry(irq, entry);
    }

    /// Set IRQ redirection entry with explicit destination APIC ID.
    pub fn set_irq_with_dest(
        &self,
        irq: u8,
        vector: u8,
        masked: bool,
        level_triggered: bool,
        active_low: bool,
        dest_apic_id: u32,
    ) {
        let mut entry: u64 = vector as u64;
        entry |= ENTRY_FIXED as u64;
        if level_triggered {
            entry |= ENTRY_LEVEL_TRIGGERED as u64;
        }
        if active_low {
            entry |= ENTRY_ACTIVE_LOW as u64;
        }
        if masked {
            entry |= ENTRY_MASKED as u64;
        }
        entry |= (dest_apic_id as u64) << 56;
        self.write_redir_entry(irq, entry);
    }

    /// Mask (disable) an IRQ pin.
    pub fn mask_irq(&self, irq: u8) {
        let entry = self.read_redir_entry(irq);
        self.write_redir_entry(irq, entry | ENTRY_MASKED as u64);
    }

    /// Unmask (enable) an IRQ pin.
    pub fn unmask_irq(&self, irq: u8) {
        let entry = self.read_redir_entry(irq);
        self.write_redir_entry(irq, entry & !(ENTRY_MASKED as u64));
    }

    /// Initialize the IOAPIC. All IRQs are masked at this point.
    /// External IRQs (keyboard, UART) will be unmasked later via
    /// unmask_external_irqs() after the first syscall.
    pub fn init(&self) {
        let version = self.version();
        let max_irq = self.max_entries();
        crate::console_println!(
            "[ioapic] IOAPIC ID={}, version={}, max_entries={}",
            self.id(),
            version & 0xFF,
            max_irq
        );

        // Mask all IRQs — external IRQs are unmasked by unmask_external_irqs()
        // after the user program's first syscall, to prevent spurious interrupts
        // during kernel boot and first_enter_user.
        for irq in 0..max_irq {
            self.set_irq(
                irq as u8,
                super::idt::IRQ_BASE + irq as u8,
                true,
                false,
                false,
            );
        }
    }

    /// Unmask external device IRQs (keyboard, UART) after the system is ready.
    pub fn unmask_external_irqs(&self) {
        let bsp_id = crate::arch::lapic::lapic_id();

        // Unmask IRQ 1: Keyboard (edge-triggered, active-high)
        self.set_irq_with_dest(1, super::idt::KEYBOARD_VECTOR, false, false, false, bsp_id);

        // Unmask IRQ 4: COM1 UART (edge-triggered, active-high)
        self.set_irq_with_dest(4, super::idt::COM1_VECTOR, false, false, false, bsp_id);

        crate::console_println!(
            "[ioapic] Unmasked IRQ1(keyboard) → v{}, IRQ4(COM1) → v{}",
            super::idt::KEYBOARD_VECTOR,
            super::idt::COM1_VECTOR
        );
    }
}

/// Global IOAPIC initialization.
pub fn init() {
    IOAPIC_BASE.store(
        probe_ioapic_address(),
        core::sync::atomic::Ordering::Relaxed,
    );
    let ioapic = IoApic::new();
    ioapic.init();
}

/// Unmask external device IRQs after the system is ready.
pub fn unmask_external_irqs() {
    let ioapic = IoApic::new();
    ioapic.unmask_external_irqs();
}

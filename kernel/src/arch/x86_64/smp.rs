//! SMP (Symmetric Multiprocessing) support for x86_64.
//!
//! On x86_64, secondary CPUs (APs) are started via LAPIC INIT/SIPI IPIs.
//! The BSP (Bootstrap Processor) sends a startup IPI pointing to a real-mode
//! trampoline, which transitions the AP to long mode and calls an entry function.
//!
//! Current status: single-core stub. Full SMP implementation requires:
//! 1. AP trampoline code in low physical memory (real mode → long mode)
//! 2. Per-CPU GDT, IDT, TSS, kernel stack
//! 3. LAPIC initialization on each AP
//! 4. GS segment base for per-CPU data access

use core::sync::atomic::{AtomicUsize, Ordering};

/// Number of active CPUs.
static ACTIVE_CPUS: AtomicUsize = AtomicUsize::new(1);

/// BSP (Bootstrap Processor) LAPIC ID.
static BSP_LAPIC_ID: AtomicUsize = AtomicUsize::new(0);

/// Initialize the BSP (Bootstrap Processor).
/// Called once during early boot on the primary CPU.
pub fn init_bsp(_cpu_id: usize) {
    // Record BSP's LAPIC ID
    let lapic_id = crate::arch::lapic::lapic_id();
    BSP_LAPIC_ID.store(lapic_id as usize, Ordering::Relaxed);
    ACTIVE_CPUS.store(1, Ordering::Relaxed);

    crate::console_println!("[smp] BSP initialized: lapic_id={}", lapic_id);
}

/// Start secondary CPUs (APs — Application Processors).
///
/// On x86_64, this involves:
/// 1. Copy AP trampoline code to a low physical address (below 1MB)
/// 2. Send INIT IPI to target LAPIC
/// 3. Wait 10ms
/// 4. Send STARTUP IPI with trampoline page
/// 5. Wait for AP to signal it's alive
///
/// Currently a stub — only BSP runs.
pub fn start_secondary_harts(total: usize) {
    if total <= 1 {
        crate::console_println!("[smp] Single core mode (BSP only)");
        return;
    }

    // TODO: Implement AP startup via LAPIC SIPI
    // For now, just log the intent
    crate::console_println!(
        "[smp] Would start {} secondary CPUs (not yet implemented)",
        total - 1
    );
}

/// Get the current CPU's LAPIC ID.
/// In single-core mode, always returns 0 (BSP).
pub fn current_hart() -> usize {
    // TODO: Read from GS base (per-CPU data) or LAPIC ID register
    0
}

/// Get the number of currently active CPUs.
pub fn active_hart_count() -> usize {
    ACTIVE_CPUS.load(Ordering::Relaxed)
}

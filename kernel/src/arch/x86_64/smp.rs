//! SMP (Symmetric Multiprocessing) support for x86_64.
//!
//! On x86_64, secondary CPUs (APs) are started via LAPIC INIT/SIPI IPIs.
//! The BSP (Bootstrap Processor) sends a startup IPI pointing to a real-mode
//! trampoline at physical address 0x7000, which transitions the AP to long mode
//! and calls `secondary_cpu_entry()`.
//!
//! Each AP gets its own:
//! - Kernel stack (allocated from PMM)
//! - GDT + TSS (per-CPU, indexed by CPU ID)
//! - LAPIC (same MMIO base, independent registers)
//! - IDT (shared with BSP — same handlers)

use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::mm::pmm;

/// Trampoline target physical address (page 0x07 = 0x7000).
const TRAMPOLINE_ADDR: usize = 0x7000;
const TRAMPOLINE_VECTOR: u8 = 0x07;

/// AP kernel stack size: 4 pages (16 KB).
const AP_STACK_PAGES: usize = 4;

/// Number of active CPUs.
static ACTIVE_CPUS: AtomicUsize = AtomicUsize::new(1);

/// BSP (Bootstrap Processor) LAPIC ID.
static BSP_LAPIC_ID: AtomicUsize = AtomicUsize::new(0);

/// Per-CPU LAPIC IDs (indexed by CPU ID).
static CPU_LAPIC_IDS: [AtomicUsize; 4] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

/// AP stack tops (indexed by CPU ID, 0 = BSP which uses its own stack).
static mut AP_STACKS: [usize; 4] = [0; 4];

/// extern declarations for trampoline symbols (defined in ap_trampoline.S).
unsafe extern "C" {
    static ap_trampoline_start: u8;
    static ap_trampoline_data: u8;
    static ap_trampoline_end: u8;
}

/// Trampoline data offsets (relative to trampoline_data).
const OFF_STACK: usize = 0;
const OFF_ENTRY: usize = 8;
const OFF_CR3: usize = 16;
const OFF_CPU_ID: usize = 24;

/// Initialize the BSP (Bootstrap Processor).
/// Called once during early boot on the primary CPU.
pub fn init_bsp(_cpu_id: usize) {
    let lapic_id = crate::arch::lapic::lapic_id();
    BSP_LAPIC_ID.store(lapic_id as usize, Ordering::Relaxed);
    CPU_LAPIC_IDS[0].store(lapic_id as usize, Ordering::Relaxed);
    ACTIVE_CPUS.store(1, Ordering::Relaxed);

    crate::console_println!("[smp] BSP initialized: lapic_id={}", lapic_id);
}

/// Start secondary CPUs (APs — Application Processors).
///
/// For each AP:
/// 1. Allocate a kernel stack
/// 2. Copy trampoline code to 0x7000
/// 3. Fill in trampoline data (stack, entry, CR3, CPU ID)
/// 4. Send INIT IPI → wait → SIPI → wait
/// 5. Wait for AP to increment ACTIVE_CPUS
pub fn start_secondary_harts(total: usize) {
    if total <= 1 {
        crate::console_println!("[smp] Single core mode (BSP only)");
        return;
    }

    let num_aps = total - 1;
    if num_aps > 3 {
        crate::console_println!(
            "[smp] Warning: requested {} APs, max supported is 3",
            num_aps
        );
    }
    let num_aps = num_aps.min(3);

    crate::console_println!("[smp] Starting {} secondary CPUs...", num_aps);

    // Get trampoline code size and copy to 0x7000
    let tramp_start = unsafe { core::ptr::addr_of!(ap_trampoline_start) as usize };
    let tramp_end = unsafe { core::ptr::addr_of!(ap_trampoline_end) as usize };
    let tramp_data = unsafe { core::ptr::addr_of!(ap_trampoline_data) as usize };
    let tramp_size = tramp_end - tramp_start;
    let data_offset = tramp_data - tramp_start; // offset of data area within trampoline

    crate::console_println!(
        "[smp] Trampoline: {:#x}..{:#x} ({} bytes), data offset={}",
        tramp_start,
        tramp_end,
        tramp_size,
        data_offset,
    );

    // Copy trampoline to physical 0x7000
    unsafe {
        ptr::copy_nonoverlapping(
            tramp_start as *const u8,
            TRAMPOLINE_ADDR as *mut u8,
            tramp_size,
        );
    }

    // Get current CR3 (kernel page table)
    let cr3 = unsafe {
        x86_64::registers::control::Cr3::read()
            .0
            .start_address()
            .as_u64() as usize
    };

    // Start each AP
    for ap_idx in 0..num_aps {
        let cpu_id = ap_idx + 1; // CPU 0 = BSP

        // Allocate AP kernel stack
        let stack_base = pmm::alloc_frame().unwrap_or(0);
        if stack_base == 0 {
            crate::console_println!("[smp] Failed to allocate stack for CPU {}", cpu_id);
            continue;
        }
        for _ in 1..AP_STACK_PAGES {
            pmm::alloc_frame();
        }
        let stack_top = stack_base + AP_STACK_PAGES * pmm::page_size();
        unsafe {
            AP_STACKS[cpu_id] = stack_top;
        }

        // Fill trampoline data area
        let data_base = TRAMPOLINE_ADDR + data_offset;
        unsafe {
            ptr::write_volatile((data_base + OFF_STACK) as *mut u64, stack_top as u64);
            ptr::write_volatile(
                (data_base + OFF_ENTRY) as *mut u64,
                secondary_cpu_entry as usize as u64,
            );
            ptr::write_volatile((data_base + OFF_CR3) as *mut u64, cr3 as u64);
            ptr::write_volatile((data_base + OFF_CPU_ID) as *mut u64, cpu_id as u64);
        }

        // Determine target LAPIC ID
        // Assume APIC IDs are sequential: BSP=0, AP1=1, AP2=2, etc.
        // This works for QEMU. Real hardware needs ACPI MADT parsing.
        let target_apic_id = cpu_id as u32;
        CPU_LAPIC_IDS[cpu_id].store(target_apic_id as usize, Ordering::Relaxed);

        crate::console_println!(
            "[smp] Starting CPU {}: lapic_id={}, stack={:#x}",
            cpu_id,
            target_apic_id,
            stack_top,
        );

        // Send INIT IPI
        crate::arch::lapic::send_init_ipi(target_apic_id);

        // Wait 10ms (use LAPIC timer or simple loop)
        // Simple delay loop for QEMU
        for _ in 0..10_000_000 {
            core::hint::spin_loop();
        }

        // Send STARTUP IPI (vector = 0x07 → AP starts at 0x7000)
        crate::arch::lapic::send_startup_ipi(target_apic_id, TRAMPOLINE_VECTOR);

        // Wait 200μs
        for _ in 0..200_000 {
            core::hint::spin_loop();
        }

        // Send second SIPI (Intel spec recommends)
        crate::arch::lapic::send_startup_ipi(target_apic_id, TRAMPOLINE_VECTOR);

        // Wait for AP to come online
        let expected = 1 + ap_idx + 1;
        let mut timeout = 10_000_000;
        while ACTIVE_CPUS.load(Ordering::Relaxed) < expected && timeout > 0 {
            core::hint::spin_loop();
            timeout -= 1;
        }

        if ACTIVE_CPUS.load(Ordering::Relaxed) >= expected {
            crate::console_println!("[smp] CPU {} is online", cpu_id);
        } else {
            crate::console_println!("[smp] CPU {} failed to start (timeout)", cpu_id);
        }
    }

    crate::console_println!(
        "[smp] All CPUs online: {}/{}",
        ACTIVE_CPUS.load(Ordering::Relaxed),
        total
    );
}

/// AP (Application Processor) entry point.
///
/// Called from `ap_trampoline.S` after transitioning to 64-bit long mode.
/// `cpu_id` is passed via RDI (set in trampoline data).
///
/// This function:
/// 1. Sets up per-CPU GDT + TSS
/// 2. Loads the shared IDT
/// 3. Initializes this CPU's LAPIC
/// 4. Enables the LAPIC timer
/// 5. Signals that this AP is online
/// 6. Enters the scheduling loop
#[unsafe(no_mangle)]
extern "C" fn secondary_cpu_entry(cpu_id: usize) -> ! {
    crate::console_println!("[smp] CPU {} entering secondary_cpu_entry", cpu_id);

    crate::arch::cet::disable();

    // Initialize per-CPU GDT + TSS
    crate::arch::gdt::init_for_cpu(cpu_id);

    // Load the shared IDT
    crate::arch::idt::load();

    // Initialize this CPU's LAPIC
    crate::arch::lapic::init();
    crate::arch::lapic::enable_timer();

    crate::console_println!("[smp] CPU {} initialized (LAPIC + GDT + IDT)", cpu_id);

    // Signal that this AP is online
    ACTIVE_CPUS.fetch_add(1, Ordering::Relaxed);

    // Enable interrupts
    x86_64::instructions::interrupts::enable();

    // Enter the scheduling loop
    crate::console_println!("[smp] CPU {} entering scheduler", cpu_id);
    loop {
        crate::sched::schedule();
        x86_64::instructions::hlt();
    }
}

/// Get the current CPU's hart ID.
/// On x86_64, reads the LAPIC ID and maps it to a CPU index.
pub fn current_hart() -> usize {
    let my_lapic_id = crate::arch::lapic::lapic_id() as usize;
    for i in 0..4 {
        if CPU_LAPIC_IDS[i].load(Ordering::Relaxed) == my_lapic_id {
            return i;
        }
    }
    0 // Default to BSP
}

/// Get the number of currently active CPUs.
pub fn active_hart_count() -> usize {
    ACTIVE_CPUS.load(Ordering::Relaxed)
}

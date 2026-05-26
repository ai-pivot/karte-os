// kernel/src/arch/smp.rs
// Symmetric Multiprocessing (SMP) support for RISC-V

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sync::spinlock::SpinLock;

/// Maximum number of harts supported
const MAX_HARTS: usize = 8;

/// Per-hart status
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HartState {
    Stopped,
    Starting,
    Running,
}

/// Per-hart data
struct HartData {
    state: HartState,
    stack_top: usize,
}

impl HartData {
    const fn new() -> Self {
        Self {
            state: HartState::Stopped,
            stack_top: 0,
        }
    }
}

static HART_DATA: SpinLock<[HartData; MAX_HARTS]> = SpinLock::new({
    const DATA: HartData = HartData::new();
    [DATA; MAX_HARTS]
});

/// Number of active harts
static ACTIVE_HARTS: AtomicUsize = AtomicUsize::new(1);

// External symbol defined by the assembly trampoline below
unsafe extern "C" {
    fn _secondary_hart_trampoline();
}

/// Secondary hart stacks, exposed to assembly trampoline.
/// Each entry is the stack top for hart N (0 = not allocated).
#[used]
#[unsafe(no_mangle)]
static SECONDARY_STACKS: [AtomicUsize; MAX_HARTS] = {
    const ZERO: AtomicUsize = AtomicUsize::new(0);
    [ZERO; MAX_HARTS]
};

/// Get the current hart ID (stored in tp register)
pub fn current_hart() -> usize {
    let hartid: usize;
    unsafe {
        core::arch::asm!("mv {}, tp", out(reg) hartid);
    }
    hartid
}

/// Initialize SMP for hart 0 (BSP — Bootstrap Processor)
pub fn init_bsp(hartid: usize) {
    // Store hartid in tp register for per-hart identification
    unsafe {
        core::arch::asm!("mv tp, {}", in(reg) hartid);
    }

    let mut data = HART_DATA.lock();
    data[hartid].state = HartState::Running;
    ACTIVE_HARTS.store(1, Ordering::Relaxed);

    crate::console_println!("[smp] BSP (hart {}) initialized", hartid);
}

/// Start secondary harts via SBI HSM hart_start
pub fn start_secondary_harts(total_harts: usize) {
    let count = total_harts.min(MAX_HARTS);

    for hartid in 1..count {
        // Allocate 4 pages (16 KB) for the secondary hart stack
        let stack_pages = 4;
        let mut stack_base = 0usize;
        let mut allocated = 0;

        for _ in 0..stack_pages {
            match crate::mm::pmm::alloc_frame() {
                Some(f) => {
                    if allocated == 0 {
                        stack_base = f;
                    }
                    allocated += 1;
                }
                None => {
                    crate::console_println!(
                        "[smp] Failed to allocate stack for hart {}",
                        hartid
                    );
                    break;
                }
            }
        }

        if allocated < stack_pages {
            crate::console_println!(
                "[smp] Skipping hart {} — not enough memory for stack",
                hartid
            );
            continue;
        }

        let stack_top = stack_base + stack_pages * crate::mm::pmm::page_size();

        {
            let mut data = HART_DATA.lock();
            data[hartid].stack_top = stack_top;
            data[hartid].state = HartState::Starting;
        }

        // Store stack top where assembly trampoline can find it
        SECONDARY_STACKS[hartid].store(stack_top, Ordering::Relaxed);

        // Use SBI to start the hart — entry point is the assembly trampoline
        let entry_addr = _secondary_hart_trampoline as *const () as usize;
        let result = sbi_rt::hart_start(hartid, entry_addr, hartid);

        if result.is_ok() {
            crate::console_println!("[smp] Started hart {}", hartid);
        } else {
            crate::console_println!(
                "[smp] Failed to start hart {}: error {:?}",
                hartid,
                result
            );
        }
    }
}

// Secondary hart trampoline (assembly).
//
// When SBI starts a secondary hart, it jumps here in supervisor mode with:
//   a0 = hartid
//   a1 = opaque (we pass hartid)
//   satp = 0 (MMU off)
//   sstatus.SIE = 0
//
// The trampoline loads the pre-allocated stack for this hart from
// the SECONDARY_STACKS array, sets up sp, then calls the Rust entry point.
core::arch::global_asm!(
    "
    .section .text
    .global _secondary_hart_trampoline
_secondary_hart_trampoline:
    // a0 = hartid (from SBI)
    // a1 = opaque (= hartid)

    // Load stack pointer from SECONDARY_STACKS[hartid]
    // SECONDARY_STACKS is an array of AtomicUsize (= usize on rv64)
    // Each element is 8 bytes.
    la      t0, SECONDARY_STACKS
    slli    t1, a0, 3          // t1 = hartid * 8
    add     t0, t0, t1         // t0 = &SECONDARY_STACKS[hartid]
    ld      t0, 0(t0)          // t0 = stack_top

    // If stack_top is 0, park this hart
    beqz    t0, .smp_park

    // Set up stack pointer
    mv      sp, t0

    // Store hartid in tp for current_hart()
    mv      tp, a0

    // Call the Rust secondary hart entry point
    // a0 = hartid (already set)
    call    secondary_hart_entry

.smp_park:
    wfi
    j       .smp_park
"
);

/// Secondary hart entry point (called from assembly trampoline).
///
/// At this point the stack is set up and tp = hartid.
#[unsafe(no_mangle)]
extern "C" fn secondary_hart_entry(hartid: usize) -> ! {
    // hartid is already in tp (set by trampoline), but re-affirm it
    unsafe {
        core::arch::asm!("mv tp, {}", in(reg) hartid);
    }

    // Verify stack was allocated
    let stack_top = {
        let data = HART_DATA.lock();
        data[hartid].stack_top
    };

    if stack_top == 0 {
        crate::console_println!("[smp] Hart {} has no stack, parking", hartid);
        loop {
            unsafe { core::arch::asm!("wfi") };
        }
    }

    // Initialize trap handling for this hart
    crate::arch::trap::init();
    crate::arch::trap::enable_timer_interrupt();
    crate::arch::trap::set_next_timer();

    // Initialize PLIC for this hart
    crate::arch::plic::init(hartid);

    // Mark as running
    {
        let mut data = HART_DATA.lock();
        data[hartid].state = HartState::Running;
    }
    ACTIVE_HARTS.fetch_add(1, Ordering::Relaxed);

    // Enable interrupts
    unsafe { riscv::register::sstatus::set_sie() };

    crate::console_println!("[smp] Hart {} online", hartid);

    // Park — timer interrupts will handle scheduling
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}

/// Get the number of active harts
pub fn active_hart_count() -> usize {
    ACTIVE_HARTS.load(Ordering::Relaxed)
}

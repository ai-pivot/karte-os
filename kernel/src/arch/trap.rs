//! Trap handling: U-mode ↔ S-mode transitions, exception dispatch, timer interrupts.

use core::arch::global_asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use riscv::register::stvec;

// Embed the trap entry/exit assembly (dual-path: S-mode + U-mode)
global_asm!(include_str!("trap_entry.S"));

unsafe extern "C" {
    fn trap_entry();
    fn trap_return_user();
}

/// Trap context: complete register state saved on trap entry.
///
/// Layout (280 bytes on stack):
///   x[0..32]  @ offset 0..256   — 32 general-purpose registers (x0 unused, x2 = sp)
///   sstatus   @ offset 256
///   sepc      @ offset 264
///   sscratch  @ offset 272      — saved sscratch (user sp if from U-mode)
#[repr(C)]
#[derive(Debug)]
pub struct TrapContext {
    pub x: [usize; 32],
    pub sstatus: usize,
    pub sepc: usize,
    pub sscratch: usize, // If from U-mode: contains user sp; else 0
}

impl TrapContext {
    /// Create a trap context for first entry into a user process.
    pub fn new_for_user(entry: usize, user_sp: usize, kernel_sp: usize) -> Self {
        let mut ctx = Self {
            x: [0; 32],
            sstatus: 0,
            sepc: entry,
            sscratch: user_sp,
        };
        // Set sstatus: SPP=0 (return to U-mode), SPIE=1 (enable interrupts after sret)
        ctx.sstatus = 0x20; // SPIE bit
        ctx.x[2] = kernel_sp; // kernel sp (used during trap handling)
        ctx
    }
}

/// Get the address of the trap_return_user assembly label.
/// Used by the scheduler to set up the return address for new tasks
/// so that __switch's ret jumps into the U-mode return path.
pub fn trap_return_user_addr() -> usize {
    unsafe { trap_return_user as *const () as usize }
}

/// Set up the trap vector.
pub fn init() {
    unsafe {
        stvec::write(stvec::Stvec::new(
            trap_entry as *const () as usize,
            stvec::TrapMode::Direct,
        ));
    }
}

/// Jump to user mode for the first time.
/// `ctx` is a TrapContext prepared on the kernel stack.
/// `user_satp` is the SATP register value for the user page table.
/// This function never returns.
#[inline(never)]
pub fn first_enter_user(ctx: &mut TrapContext, user_satp: usize) -> ! {
    unsafe {
        let user_sp = ctx.sscratch;
        let kernel_sp = ctx.x[2];
        let sstatus_val = ctx.sstatus;
        let sepc_val = ctx.sepc;

        // Single asm block: switch satp + sfence.vma + CSR setup + sret.
        // All in one block to prevent compiler from inserting instructions
        // between csrw satp and sfence.vma (which would use stale TLB).
        core::arch::asm!(
            "csrw satp, {satp}",
            "sfence.vma",
            "csrw sscratch, {usp}",
            "csrw sstatus, {st}",
            "csrw sepc, {epc}",
            "csrw sscratch, {ksp}",
            "mv sp, {usp}",
            "sret",
            satp = in(reg) user_satp,
            usp = in(reg) user_sp,
            st = in(reg) sstatus_val,
            epc = in(reg) sepc_val,
            ksp = in(reg) kernel_sp,
            options(noreturn)
        );
    }
}

/// Enable supervisor timer interrupt.
pub fn enable_timer_interrupt() {
    unsafe {
        riscv::register::sie::set_stimer();
    }
}

/// Set the next timer interrupt (10ms from now).
pub fn set_next_timer() {
    const CLOCK_FREQ: usize = 10_000_000;
    const TICKS_PER_MS: usize = CLOCK_FREQ / 1000;
    let next = riscv::register::time::read() + 10 * TICKS_PER_MS;
    let _ = ::sbi::timer::set_timer(next as u64);
}

/// Get the current process's page table.
/// Returns the user page table if a process is active, otherwise falls back to the kernel page table.
pub fn get_current_user_pt() -> &'static mut crate::mm::vmm::PageTable {
    let ppn = crate::process::current_page_table_ppn();
    if ppn == 0 {
        // Phase 1: shared kernel page table (no separate per-process PT yet)
        crate::mm::vmm::get_kernel_page_table()
    } else {
        unsafe { &mut *((ppn << 12) as *mut crate::mm::vmm::PageTable) }
    }
}

static TIMER_TICKS: AtomicUsize = AtomicUsize::new(0);

fn handle_timer() {
    let ticks = TIMER_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    if ticks % 100 == 0 {
        crate::console_println!("[timer] tick {} ({}s)", ticks, ticks / 100);
    }
    set_next_timer();
    crate::sched::schedule();
}

/// Trap handler — called from assembly trap_entry.
#[unsafe(no_mangle)]
extern "C" fn trap_handler(ctx: &mut TrapContext) -> &mut TrapContext {
    let scause = riscv::register::scause::read();
    let stval = riscv::register::stval::read();

    // Decode scause from raw bits
    let scause_code = scause.bits();
    let is_interrupt = scause_code & (1 << 63) != 0;
    let code = scause_code & !(1 << 63);

    // If from U-mode, enable SUM so S-mode can access user pages
    let from_user = ctx.sscratch != 0;
    if from_user {
        unsafe { riscv::register::sstatus::set_sum() };
    }

    if is_interrupt {
        match code {
            5 => handle_timer(), // Supervisor Timer
            9 => {
                // Supervisor External (PLIC)
                crate::arch::plic::handle_interrupt(0);
            }
            _ => {
                // Unknown S-mode interrupt — skip instruction.
                // Do NOT print — SpinLock in UART can deadlock.
                skip_trap_instruction(ctx);
            }
        }
    } else {
        match code {
            8 => {
                // User Environment Call (ecall from U-mode)
                // a7 = syscall number (x[17])
                // a0-a5 = args (x[10]-x[15])
                let syscall_id = ctx.x[17];
                let args = [
                    ctx.x[10], ctx.x[11], ctx.x[12], ctx.x[13], ctx.x[14], ctx.x[15],
                ];
                let result = crate::syscall::dispatch(syscall_id, args);
                ctx.x[10] = result as usize; // a0 = return value
                ctx.sepc += 4; // skip ecall instruction
            }
            2 => {
                // Illegal instruction — silently skip.
                // This handles CSR probing by the Rust runtime (sstateen0, senvcfg,
                // stimecmp, etc.) which are not supported by QEMU -cpu rv64.
                // We must NOT print here as console_println acquires a SpinLock
                // which can deadlock if a timer interrupt fires during the lock hold.
                skip_trap_instruction(ctx);
            }
            5 | 7 => {
                // Load/Store Access Fault
                // Silently skip — happens when probing unmapped VirtIO devices
                skip_trap_instruction(ctx);
            }
            12 | 13 | 15 => {
                // Instruction/Load/Store page fault
                let fault_addr = stval;
                let heap_base = crate::process::USER_HEAP_BASE;
                let heap_limit = crate::process::USER_HEAP_LIMIT;

                // Check if fault is in user heap area (lazy allocation)
                if from_user && fault_addr >= heap_base && fault_addr < heap_limit {
                    // Lazy page allocation: map a new page at the faulting address
                    let page_size = crate::mm::pmm::page_size();
                    let page_addr = fault_addr & !(page_size - 1); // Align down

                    let kernel_pt = get_current_user_pt();

                    // Check if already mapped (shouldn't be, but safety check)
                    if crate::mm::vmm::translate_user(kernel_pt, page_addr).is_none() {
                        if let Some(frame) = crate::mm::pmm::alloc_frame() {
                            unsafe {
                                core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                            }
                            crate::mm::vmm::map(
                                kernel_pt,
                                page_addr,
                                frame,
                                crate::mm::vmm::PTEFlags::URW,
                            );
                            unsafe {
                                core::arch::asm!("sfence.vma");
                            }
                            // Update brk if needed
                            let new_brk = page_addr + page_size;
                            if new_brk > crate::process::current_brk() {
                                crate::process::set_current_brk(new_brk);
                            }
                            // Don't advance sepc — retry the faulting instruction
                        }
                        // else: OOM — fall through to not advancing sepc (still retry)
                    }
                    // Page already mapped or just allocated — don't advance sepc, retry instruction
                    // (no ctx.sepc modification)
                } else {
                    // Not in heap area — fatal page fault
                    crate::console_println!(
                        "[trap] Page fault (code={}) at sepc={:#x}, stval={:#x}",
                        code,
                        ctx.sepc,
                        stval
                    );
                    if from_user {
                        crate::console_println!("[trap] Killing user process");
                        crate::syscall::dispatch(1, [1, 0, 0, 0, 0, 0]); // sys_exit(1)
                    } else {
                        skip_trap_instruction(ctx);
                    }
                }
            }
            _ => {
                // Unknown exception — silently skip.
                // This handles CSR probing and other spurious exceptions
                // that may occur during runtime initialization.
                skip_trap_instruction(ctx);
            }
        }
    }

    // Clear SUM if we set it for user-mode trap handling
    if from_user {
        unsafe { riscv::register::sstatus::clear_sum() };
    }

    // Restore the correct page table for the current process.
    // After schedule()/__switch, we may be running on a different process
    // with a different user page table. CURRENT_PAGE_TABLE_ROOT was updated
    // by the scheduler before __switch.
    if from_user {
        let target_ppn = crate::process::current_page_table_root();
        if target_ppn != 0 {
            let current_satp = riscv::register::satp::read().bits();
            let current_ppn = current_satp & ((1usize << 44) - 1);
            if target_ppn != current_ppn {
                let new_satp = (8usize << 60) | target_ppn;
                unsafe {
                    core::arch::asm!("csrw satp, {}", in(reg) new_satp);
                    core::arch::asm!("sfence.vma");
                }
            }
        }
    }

    ctx
}

/// Skip the faulting instruction by advancing sepc.
/// Handles both 16-bit (compressed) and 32-bit RISC-V instructions.
fn skip_trap_instruction(ctx: &mut TrapContext) {
    // Always advance by 4 bytes. Reading the instruction at sepc to
    // determine compressed vs standard length (read_volatile) can cause
    // a page fault if sepc is in unmapped memory (e.g., OpenSBI region
    // during CSR probing). All our code and the Rust runtime use standard
    // 32-bit instructions for CSR probes, so 4 is always correct.
    ctx.sepc += 4;
}

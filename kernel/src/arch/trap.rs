//! Trap handling: U-mode ↔ S-mode transitions, exception dispatch, timer interrupts.

use core::arch::global_asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use riscv::register::stvec;

// Embed the trap entry/exit assembly (dual-path: S-mode + U-mode)
global_asm!(include_str!("trap_entry.S"));

unsafe extern "C" {
    fn trap_entry();
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
    ///
    /// - `entry`     — user program entry point (set in sepc)
    /// - `user_sp`   — user stack pointer (set in sscratch)
    /// - `kernel_sp` — kernel stack pointer (set in x[2], used during trap handling)
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
pub fn first_enter_user(ctx: &mut TrapContext, user_satp: usize) -> ! {
    unsafe {
        // Switch to user page table
        core::arch::asm!("csrw satp, {}", in(reg) user_satp);
        core::arch::asm!("sfence.vma");

        // Set sscratch = user_sp for future U-mode traps
        core::arch::asm!("csrw sscratch, {}", in(reg) ctx.sscratch);
        // Set sstatus: SPP=0 (return to U-mode), SPIE=1 (enable intr after sret)
        core::arch::asm!("csrw sstatus, {}", in(reg) ctx.sstatus);
        // Set sepc = user entry point
        core::arch::asm!("csrw sepc, {}", in(reg) ctx.sepc);
        // Set user stack pointer (sp will be user_sp after swap)
        // We use the same trick as trap return: swap sp with sscratch
        // But we need to also set sscratch = kernel sp for next trap
        let user_sp = ctx.sscratch;
        let kernel_sp = ctx.x[2]; // kernel stack top
        core::arch::asm!(
            "csrw sscratch, {ksp}",
            "mv sp, {usp}",
            "sret",
            ksp = in(reg) kernel_sp,
            usp = in(reg) user_sp,
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
                crate::console_println!(
                    "[trap] Unknown interrupt: code={}, sepc={:#x}",
                    code,
                    ctx.sepc
                );
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
                // Illegal instruction
                crate::console_println!("[trap] Illegal instruction at sepc={:#x}", ctx.sepc);
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
                crate::console_println!(
                    "[trap] Exception code={} sepc={:#x} stval={:#x}",
                    code,
                    ctx.sepc,
                    stval
                );
                skip_trap_instruction(ctx);
            }
        }
    }

    // Clear SUM if we set it for user-mode trap handling
    if from_user {
        unsafe { riscv::register::sstatus::clear_sum() };
    }

    ctx
}

/// Skip the faulting instruction by advancing sepc.
/// Handles both 16-bit (compressed) and 32-bit RISC-V instructions.
fn skip_trap_instruction(ctx: &mut TrapContext) {
    // In RISC-V, compressed (16-bit) instructions have bits[1:0] != 0b11.
    // We need to read the instruction at sepc from the kernel's perspective.
    // Since sepc points to kernel code (identity mapped), we can read it directly.
    let pc = ctx.sepc;
    let instr_half = unsafe { core::ptr::read_volatile(pc as *const u16) };
    let len = if instr_half & 0x3 != 0x3 { 2 } else { 4 };
    ctx.sepc += len;
}

//! Trap handling for x86_64: context save/restore, syscall dispatch, iretq.
//!
//! On RISC-V, traps are handled by `trap_entry.S` which saves all 32 GP regs +
//! CSRs into a TrapContext struct. On x86_64, the CPU pushes an InterruptStackFrame
//! (SS, RSP, RFLAGS, CS, RIP) automatically on privilege level changes, and the
//! `extern "x86-interrupt"` ABI saves additional callee-saved registers.
//!
//! For syscalls and context switching we need a FULL register save (TrapContext)
//! because the scheduler needs to restore arbitrary register state when switching
//! tasks. We use `naked` functions + inline `asm!` for iretq and CR3 switching.
//!
//! # TrapContext Stack Layout
//!
//! ```text
//! Offset  Field           Notes
//! ──────  ─────           ─────
//! 0x00    rax             ← pop rax  (rsp starts here)
//! 0x08    rbx             ← pop rbx
//! 0x10    rcx             ← pop rcx
//! 0x18    rdx             ← pop rdx
//! 0x20    rbp             ← pop rbp
//! 0x28    rsi             ← pop rsi
//! 0x30    rdi             ← pop rdi
//! 0x38    r8              ← pop r8
//! 0x40    r9              ← pop r9
//! 0x48    r10             ← pop r10
//! 0x50    r11             ← pop r11
//! 0x58    r12             ← pop r12
//! 0x60    r13             ← pop r13
//! 0x68    r14             ← pop r14
//! 0x70    r15             ← pop r15
//! 0x78    rip             ← iretq frame (consumed by iretq)
//! 0x80    cs
//! 0x88    rflags
//! 0x90    rsp             (user RSP)
//! 0x98    ss
//! 0xA0    kernel_sp       ← kernel metadata (not popped)
//! 0xA8    user_cr3         page table to switch to (0 = no switch)
//! 0xB0    trap_from_user   non-zero if trap originated from Ring 3
//! ```
//!
//! After popping the 15 GP regs (14 bytes × 8 = 0x78), rsp points to the
//! iretq frame, which `iretq` consumes directly. The kernel metadata at
//! 0xA0..0xB7 is NOT on the iretq path.

use core::sync::atomic::Ordering;

use x86_64::registers::control::Cr3;
use x86_64::structures::paging::PhysFrame;
use x86_64::{PhysAddr, VirtAddr};

/// Complete register state saved on trap entry.
///
/// Field ordering is critical: GP regs first, then iretq frame, then
/// kernel extras. This matches what `trap_return_user` and the scheduler's
/// `add_user_process` expect.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct TrapContext {
    // ── General-purpose registers (offset 0x00..0x78) ──
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rbp: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    // ── iretq frame (offset 0x78..0xA0) ──
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
    // ── Kernel metadata (offset 0xA0..0xB8) ──
    pub kernel_sp: u64,
    pub user_cr3: u64,
    pub trap_from_user: u64,
}

/// Size of TrapContext in bytes (184 bytes = 23 × 8).
pub const TRAP_CONTEXT_SIZE: usize = core::mem::size_of::<TrapContext>();

// Compile-time check: 15 GP regs × 8 = 120 = 0x78 = offset of iretq frame
const _: () = assert!(core::mem::offset_of!(TrapContext, rip) == 15 * 8);

impl TrapContext {
    /// Create a TrapContext for first entry into a user process.
    /// Mirrors RISC-V's `TrapContext::new_for_user`.
    pub fn new_for_user(entry: usize, user_sp: usize, kernel_sp: usize) -> Self {
        Self {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rbp: 0,
            rsi: 0,
            rdi: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: entry as u64,
            cs: super::gdt::USER_CODE_SEL.load(Ordering::Relaxed) as u64,
            rflags: 0x202, // IF (interrupt enable) + reserved bit 1
            rsp: user_sp as u64,
            ss: super::gdt::USER_DATA_SEL.load(Ordering::Relaxed) as u64,
            kernel_sp: kernel_sp as u64,
            user_cr3: 0, // Set by caller before entering user mode
            trap_from_user: 0,
        }
    }
}

/// Get the address of `trap_return_user` — the assembly label that
/// `__switch`'s `ret` jumps into to return to user mode.
///
/// On x86_64, `trap_return_user` is implemented as a naked function
/// that restores GP registers and executes `iretq` back to Ring 3.
pub fn trap_return_user_addr() -> usize {
    trap_return_user as usize
}

/// Assembly trampoline that returns from kernel to user mode via iretq.
///
/// When the scheduler switches to a task for the first time, `__switch`
/// returns (via `ret`) to this function. The stack has been set up by
/// `add_user_process` with a TrapContext containing the GP regs and
/// iretq frame.
///
/// # Stack on entry
///
/// RSP points to the base of a TrapContext. After 15 `pop` instructions
/// (one per GP reg), RSP lands on the iretq frame (RIP, CS, RFLAGS, RSP, SS).
/// `iretq` then atomically restores all five and switches to Ring 3.
///
/// The kernel metadata fields (kernel_sp, user_cr3, trap_from_user) are at
/// higher addresses and are never touched by the pop/iretq sequence.
///
/// # CR3 switching
///
/// If `user_cr3` is non-zero, we switch to the user page table before iretq.
/// This is set only for a task's very first entry into U-mode (via
/// `add_user_process`). Normal trap returns leave it 0.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn trap_return_user() {
    unsafe {
        core::arch::naked_asm!(
            // ── Restore general-purpose registers ──
            "pop rax",
            "pop rbx",
            "pop rcx",
            "pop rdx",
            "pop rbp",
            "pop rsi",
            "pop rdi",
            "pop r8",
            "pop r9",
            "pop r10",
            "pop r11",
            "pop r12",
            "pop r13",
            "pop r14",
            "pop r15",
            // After 15 pops, rsp now points to the iretq frame (rip field).
            // Layout from rsp:
            //   +0:  rip        (+0x00)
            //   +8:  cs         (+0x08)
            //   +16: rflags     (+0x10)
            //   +24: rsp        (+0x18)
            //   +32: ss         (+0x20)
            //   +40: kernel_sp  (+0x28)
            //   +48: user_cr3   (+0x30) ← per-process page table root

            // ── Switch to user page table if user_cr3 is set ──
            // CR3 write implicitly flushes the TLB (non-global pages).
            // cli ensures no interrupt fires between CR3 write and iretq.
            "cli",
            "cmp qword ptr [rsp + 0x30], 0",
            "je 2f",                 // skip if user_cr3 == 0
            "mov rax, [rsp + 0x30]", // load user_cr3 (physical address of user PT root)
            "mov cr3, rax",          // switch to user page table
            "2:",
            // ── Return to Ring 3 ──
            "iretq",
        );
    }
}

/// Initialize trap handling: set up GDT, IDT, and configure the LAPIC.
pub fn init() {
    // GDT must be initialized before IDT (TSS selector needed for double fault IST)
    super::gdt::init();
    super::idt::init();
    super::lapic::init();
    super::lapic::enable_timer();
}

/// Enable timer interrupts via LAPIC.
pub fn enable_timer_interrupt() {
    super::lapic::enable_timer();
}

/// Set the next timer interrupt (periodic mode — no-op on x86_64).
pub fn set_next_timer() {
    super::lapic::set_next_timer();
}

/// Jump to user mode for the first time.
///
/// This is the x86_64 equivalent of RISC-V's `first_enter_user`.
/// It sets TSS.RSP0, switches to the user page table (CR3), builds
/// an iretq frame on the kernel stack, and executes `iretq` to
/// transition from Ring 0 to Ring 3.
///
/// # Arguments
/// - `entry`: User-mode entry point (virtual address)
/// - `user_sp`: User-mode stack pointer
/// - `kernel_sp`: Kernel stack top (used for TSS.RSP0)
/// - `user_cr3`: Physical address of user PML4 table
pub fn first_enter_user(entry: usize, user_sp: usize, kernel_sp: usize, user_cr3: u64) -> ! {
    let user_cs = super::gdt::USER_CODE_SEL.load(Ordering::Relaxed) as u64;
    let user_ss = super::gdt::USER_DATA_SEL.load(Ordering::Relaxed) as u64;

    // Set TSS.RSP0 so Ring 3 → Ring 0 interrupts use the kernel stack
    unsafe {
        super::gdt::set_kernel_rsp0(kernel_sp as u64);
    }

    unsafe {
        core::arch::asm!(
            // Disable interrupts during the critical iretq sequence.
            "cli",

            // ── Switch to user page table ──
            // user_cr3 is the physical address of the user PML4 table.
            // If non-zero, switch CR3 before building the iretq frame.
            "cmp {cr3}, 0",
            "je 2f",
            "mov rax, {cr3}",
            "mov cr3, rax",       // switch to per-process page table
            "2:",

            // Use kernel stack to build iretq frame
            "mov rsp, {ksp}",

            // Push iretq frame (must be in this exact order for iretq):
            //   SS, RSP, RFLAGS, CS, RIP  (pushed bottom-to-top)
            "push {ss}",
            "push {usp}",
            "push 0x202",             // RFLAGS: IF=1, reserved bit 1=1
            "push {cs}",
            "push {entry}",

            // Clear all GP registers (prevent kernel data leaks)
            "xor rax, rax",
            "xor rbx, rbx",
            "xor rcx, rcx",
            "xor rdx, rdx",
            "xor rsi, rsi",
            "xor rdi, rdi",
            "xor rbp, rbp",
            "xor r8,  r8",
            "xor r9,  r9",
            "xor r10, r10",
            "xor r11, r11",
            "xor r12, r12",
            "xor r13, r13",
            "xor r14, r14",
            "xor r15, r15",

            // Atomically restore RIP, CS, RFLAGS, RSP, SS → Ring 3
            "iretq",

            ksp   = in(reg) kernel_sp as u64,
            ss    = in(reg) user_ss,
            usp   = in(reg) user_sp as u64,
            cs    = in(reg) user_cs,
            entry = in(reg) entry as u64,
            cr3   = in(reg) user_cr3,
            options(noreturn),
        );
    }
}

/// Global trap handler — called from ISR stubs with a TrapContext
/// containing the full register state.
///
/// This is the x86_64 equivalent of the RISC-V `trap_handler`.
/// Dispatches syscalls and exceptions.
#[unsafe(no_mangle)]
unsafe extern "C" fn trap_handler(ctx: &mut TrapContext) -> &mut TrapContext {
    let from_user = ctx.cs & 0x3 != 0; // CPL = bottom 2 bits of CS

    if from_user {
        // Dispatch syscall: rax = syscall number, args in rdi, rsi, rdx, r10, r8, r9
        let syscall_id = ctx.rax as usize;
        let args = [
            ctx.rdi as usize,
            ctx.rsi as usize,
            ctx.rdx as usize,
            ctx.r10 as usize,
            ctx.r8 as usize,
            ctx.r9 as usize,
        ];

        // Save trap context pointer for linux_clone (needs parent register state)
        crate::process::set_trap_ctx_ptr(ctx as *mut _ as usize);

        let result = crate::syscall::dispatch(syscall_id, args);

        // Clear trap context pointer
        crate::process::set_trap_ctx_ptr(0);

        ctx.rax = result as u64;
        ctx.rip += 2; // skip `int 0x80` (2-byte instruction)
    }

    // Restore the correct page table for the current process.
    // After schedule()/__switch, we may be on a different process.
    if from_user {
        let target_root = crate::process::current_page_table_root();
        if target_root != 0 {
            let (current_frame, _) = Cr3::read();
            let current_root = current_frame.start_address().as_u64() as usize;
            if target_root != current_root {
                activate_page_table(target_root);
            }
        }
    }

    ctx
}

/// Read the current page table root (CR3 physical address).
pub fn read_page_table_root() -> usize {
    let (frame, _) = Cr3::read();
    frame.start_address().as_u64() as usize
}

/// Get the current process's page table.
/// Returns a mutable reference to the kernel page table
/// (x86_64 uses identity-mapped kernel page table as base).
pub fn get_current_user_pt() -> &'static mut crate::mm::vmm::PageTable {
    let ppn = crate::process::current_page_table_ppn();
    if ppn == 0 {
        crate::mm::vmm::get_kernel_page_table()
    } else {
        unsafe { &mut *((ppn << 12) as *mut crate::mm::vmm::PageTable) }
    }
}

/// Activate a page table by writing to CR3.
pub fn activate_page_table(root_paddr: usize) {
    let phys = PhysAddr::new(root_paddr as u64);
    let new_frame = PhysFrame::containing_address(phys);
    let (current_frame, _) = Cr3::read();
    if current_frame != new_frame {
        unsafe {
            Cr3::write(new_frame, x86_64::registers::control::Cr3Flags::empty());
        }
    }
}

/// Flush the entire TLB.
pub fn flush_tlb() {
    x86_64::instructions::tlb::flush_all();
}

/// Flush a single virtual address from the TLB.
pub fn flush_tlb_addr(addr: usize) {
    let vaddr = VirtAddr::new(addr as u64);
    x86_64::instructions::tlb::flush(vaddr);
}

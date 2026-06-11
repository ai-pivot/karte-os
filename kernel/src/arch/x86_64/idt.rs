//! ISR stubs for x86_64 trap/syscall entry with CR3 switching.
//!
//! All Ring 3→Ring 0 entry points switch CR3 from user page table to kernel
//! page table. This allows complete separation of user and kernel address spaces.

use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

static IDT: Once<InterruptDescriptorTable> = Once::new();

pub const IRQ_BASE: u8 = 32;
pub const SYSCALL_VECTOR: u8 = 0x80;
pub const TIMER_VECTOR: u8 = IRQ_BASE + 0;
pub const KEYBOARD_VECTOR: u8 = IRQ_BASE + 1;
pub const COM1_VECTOR: u8 = IRQ_BASE + 4;
pub const SPURIOUS_VECTOR: u8 = IRQ_BASE + 7;
pub const PAGE_FAULT_VECTOR: u8 = 14; // CPU exception vector for #PF

// ─── MSR constants for SYSCALL/SYSRET ─────────────────────────
const MSR_EFER: u32 = 0xC000_0080;
const MSR_STAR: u32 = 0xC000_0081;
const MSR_LSTAR: u32 = 0xC000_0082;
const MSR_CSTAR: u32 = 0xC000_0083;
const MSR_SFMASK: u32 = 0xC000_0084;
const MSR_FS_BASE: u32 = 0xC000_0100;
const MSR_GS_BASE: u32 = 0xC000_0101;
const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;

pub unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!("wrmsr", in("ecx") msr, in("eax") lo, in("edx") hi);
}

pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi);
    ((hi as u64) << 32) | (lo as u64)
}

/// Cached physical address of the kernel page table root.
#[unsafe(no_mangle)]
static mut KERNEL_CR3: u64 = 0;

/// User CR3 value saved by syscall_fast_handler for SYSCALL return path.
/// Written before dispatch, read by inline asm in SYSCALL return.
#[used]
#[cfg_attr(target_arch = "x86_64", unsafe(link_section = ".data"))]
/// Kernel stack pointer for SYSCALL fast entry.
#[unsafe(no_mangle)]
pub(crate) static mut SYSCALL_KSP: u64 = 0;
/// Temporarily saved original rbx (clobbered by user RSP during SYSCALL entry).
#[unsafe(no_mangle)]
static mut SYSCALL_SAVED_RBX: u64 = 0;
/// Temporarily saved R11 (user RFLAGS) during SYSCALL entry — r11 used as scratch
/// for per-task kernel stack lookup before being pushed to the kernel stack.
#[unsafe(no_mangle)]
static mut SYSCALL_SAVED_R11: u64 = 0;
pub static mut CLONE_DBG_RAX: u64 = 0;
pub static mut CLONE_DBG_RIP: u64 = 0;
pub static mut CLONE_DBG_RSPVAL: u64 = 0;

/// Kernel CR3 physical address, used by Timer ISR for CR3 switching.
#[used]
#[cfg_attr(target_arch = "x86_64", unsafe(link_section = ".data"))]
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static mut KERNEL_CR3_PHYS: u64 = 0;

/// Get kernel CR3 physical address. Safe to call from trap handlers.
/// Global tick counter for timekeeping
static TICK_COUNT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Get current tick count (incremented by timer ISR ~100Hz)
pub fn get_tick_count() -> usize {
    TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn get_kernel_cr3_phys() -> usize {
    unsafe { KERNEL_CR3_PHYS as usize }
}

/// Get approximate time in milliseconds since boot.
/// Based on timer interrupt tick count (~100Hz = 10ms per tick).
pub fn get_time_ms() -> u64 {
    let tick = TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed);
    tick as u64 * 10 // ~10ms per tick
}

pub fn set_syscall_ksp(ksp: u64) {
    unsafe {
        SYSCALL_KSP = ksp;
        KERNEL_CR3_PHYS = crate::mm::vmm::kernel_cr3();
    }
}

pub fn get_syscall_ksp() -> u64 {
    unsafe { SYSCALL_KSP }
}

pub fn cache_kernel_cr3() {
    unsafe {
        let kernel_cr3 = crate::mm::vmm::kernel_cr3();
        KERNEL_CR3 = kernel_cr3;
        KERNEL_CR3_PHYS = kernel_cr3;
    }
}

pub fn init_syscall_msrs() {
    unsafe {
        // Enable SYSCALL/SYSRET (SCE=bit0) and No-Execute (NXE=bit11) in EFER
        let efer = rdmsr(MSR_EFER);
        wrmsr(MSR_EFER, efer | (1 << 0) | (1 << 11));
        // Set up SYSCALL MSRs
        // SYSCALL: CS = STAR[48:63] = 0x08(kcode), SS = 0x10(kdata)
        // SYSRET:  CS = STAR[48:63]+16 = 0x18→0x1B(ucode), SS = 0x10→0x13(ok in 64-bit)
        // Both Intel and AMD SYSRET work with this value.
        let star_val = (0x08u64 << 48) | (0x08u64 << 32);
        wrmsr(MSR_STAR, star_val);
        wrmsr(MSR_LSTAR, syscall_entry as usize as u64);
        wrmsr(MSR_CSTAR, 0);
        wrmsr(MSR_SFMASK, 1 << 9); // Mask IF on syscall entry
    }
}

// ─── ISR stubs defined via global_asm! ─────────────────────────
// We use global_asm! for timer/syscall/syscall_entry/page_fault because
// naked_asm! in Rust doesn't reliably support `sym` references on nightly.
//
// NOTE: The keyboard_isr_stub and com1_isr_stub (defined below as naked
// functions) have the same 15-register save/restore sequence as
// timer_isr_stub, but with DIFFERENT push orders and handler logic:
//   - timer (global_asm!): push rax → r15  (ascending)
//   - keyboard/com1 (naked_asm!): push r15 → rax (descending)
// This means the push/pop sequences CANNOT be shared via a Rust macro.
// Additionally, naked_asm! doesn't support nested macro expansion to
// multiple string parameters, so extracting push_all/pop_all macros
// is not feasible. Keep these sequences in sync manually.

core::arch::global_asm!(
    ".section .text",
    // ─── int 0x80 syscall ISR stub ───────────────────────
    ".globl syscall_isr_stub",
    ".type syscall_isr_stub, @function",
    "syscall_isr_stub:",
    "cli",
    // Switch to kernel CR3. Syscalls execute entirely in kernel address space.
    // User memory is accessed via with_user_cr3() / UserPtr helpers.
    "mov rcx, cr3",
    "push rcx", // [bottom] user CR3 — restore before iretq
    "mov rcx, [rip + KERNEL_CR3_PHYS]",
    "mov cr3, rcx",
    // Save FPU/SSE state
    // CPU pushes 48 (has error_code) + CR3(8) + 9 GPR(72) = 128 → RSP % 16 == 0. sub 512 stays aligned.
    "sub rsp, 512",
    "fxsave64 [rsp]",
    // Save registers (9 slots, 72 bytes)
    // Stack: [0]=rax(nr) [1]=rax(ret) [2]=rdi [3]=rsi [4]=rdx [5]=r8 [6]=r9 [7]=r10 [8]=r11
    "push r11",
    "push r10",
    "push r9",
    "push r8",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rax", // [1] placeholder for return value
    "push rax", // [0] syscall number
    "mov rdi, rsp",
    "call syscall_handler_impl",
    // Store return value at slot [1] (offset 8)
    "mov [rsp + 8], rax",
    "add rsp, 8", // skip [0] (syscall nr)
    "pop rax",    // [1] (return value)
    "pop rdi",    // [2]
    "pop rsi",    // [3]
    "pop rdx",    // [4]
    "pop r8",     // [5]
    "pop r9",     // [6]
    "pop r10",    // [7]
    "pop r11",    // [8]
    // Restore FPU/SSE
    "fxrstor64 [rsp]",
    "add rsp, 512",
    "pop rcx", // user CR3
    "mov cr3, rcx",
    "iretq",
    // ─── Timer ISR stub ──────────────────────────────────
    ".globl timer_isr_stub",
    ".type timer_isr_stub, @function",
    "timer_isr_stub:",
    // Save all 15 GP registers (matches TrapContext layout)
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rbp",
    "push rsi",
    "push rdi",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    // Timer can interrupt user mode while CR3 is still a user page table.
    // Switch before calling Rust: the handler may touch kernel heap/logging.
    // It restores the post-schedule target CR3 before returning.
    "mov rax, [rip + KERNEL_CR3_PHYS]",
    "cmp rax, 0",
    "je 4f",
    "mov cr3, rax",
    "4:",
    // Save FPU/SSE state (timer can interrupt Go's SSE instructions)
    // CPU pushes 40 (no error_code) + 15 GPR(120) = 160 → RSP % 16 == 0. sub 512 stays aligned.
    "sub rsp, 512",
    "fxsave64 [rsp]",
    // Call handler
    "mov rdi, rsp",
    "call timer_trap_handler",
    // Restore FPU/SSE
    "fxrstor64 [rsp]",
    "add rsp, 512",
    // Pop 15 registers
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdi",
    "pop rsi",
    "pop rbp",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",
    "iretq",
    // ─── SYSCALL fast entry (MSR LSTAR) ──────────────────
    ".globl syscall_entry",
    ".type syscall_entry, @function",
    "syscall_entry:",
    // On SYSCALL: rcx=return RIP, r11=return RFLAGS (saved by CPU)
    //             rax=syscall_nr, rdi/rsi/rdx/r10/r8/r9=args
    //             rsp=user RSP (NOT switched), CPL=0
    //
    // CRITICAL: Use per-task kernel stack from TSS.RSP0 instead of global
    // SYSCALL_KSP. When Timer ISR fires during SYSCALL handler (after sti),
    // it saves state on the SAME kernel stack (CPL=0, IST=0 → no stack switch).
    // __switch() saves/restores per-task SPs. If multiple tasks share a global
    // SYSCALL_KSP stack, __switch restores a stale SP whose data was overwritten
    // by another task's SYSCALL. Using TSS.RSP0 (updated on every context switch)
    // ensures each task has its own kernel stack.

    // 1. Disable interrupts before touching any register or stack state.
    // SYSCALL does NOT clear IF; while CPL=0 and RSP still points at the user
    // stack, a timer IRQ would not switch to RSP0 and could corrupt user memory.
    "cli",
    // 1. Save original rbx (will be clobbered by user RSP), then switch to kernel stack
    "mov [rip + SYSCALL_SAVED_RBX], rbx",
    "mov rbx, rsp",
    // Load per-task kernel stack from TSS.RSP0.
    // TSS_RSP0_ADDR holds the address of TSS.privilege_stack_table[0].
    // Double-indirect load: [TSS_RSP0_ADDR] → ptr to RSP0, [ptr] → RSP0 value.
    "mov rsp, [rip + TSS_RSP0_ADDR]",
    "mov rsp, [rsp]",
    // Keep interrupts disabled throughout the syscall handler; timer ISR uses
    // the same SpinLock (UART) as console_println!
    //    iretq will restore RFLAGS (with IF=1) on return to user mode.
    // Do NOT switch CR3 here. The user page table has kernel identity mappings
    // (via copy_kernel_mappings). Switching CR3 would break user memory access
    // (syscall handler needs to read user buffers for paths, data, etc.).
    "push r15",
    "push r14",
    "push r13",
    "push r12",
    "push rbp",
    // 3. Push syscall args and nr
    "push r9",  // a5
    "push r8",  // a4
    "push r10", // a3
    "push rdx", // a2
    "push rsi", // a1
    "push rdi", // a0
    "push rax", // syscall_nr
    // 4. Save user return state
    "push r11", // user RFLAGS
    "push rcx", // user RIP
    "push rbx", // user RSP
    // Save original user RBX in this task's syscall frame. The global above is
    // only a bridge while switching stacks; syscalls may schedule before return.
    "push qword ptr [rip + SYSCALL_SAVED_RBX]",
    // 5. Save user CR3 and switch to kernel CR3.
    // The user page table's identity mapping may be corrupted by ELF loading,
    // so we switch to the kernel page table for the entire syscall.
    // User memory access will use with_user_cr3() helper in Rust.
    "mov rax, cr3",
    "push rax", // [0] = user CR3 (saved from hardware)
    "mov rax, [rip + KERNEL_CR3_PHYS]",
    "mov cr3, rax", // Switch to kernel page table
    // Save FPU/SSE state (kernel may use XMM registers in Rust code)
    // 17 pushes = 136 bytes → RSP % 16 == 8 (misaligned for fxsave64).
    // Sub 520 (512 + 8 padding) realigns to 16 bytes.
    "sub rsp, 520",
    "fxsave64 [rsp]",
    // Stack layout now (rsp + 520 = state_ptr passed to handler):
    //   [+0]  user_cr3
    //   [+1]  original user RBX
    //   [+2]  rbx (user RSP)
    //   [+3]  rcx (user RIP)
    //   [+4]  r11 (user RFLAGS)
    //   [+5]  rax (syscall_nr)
    //   [+6]  rdi  [+7] rsi  [+8] rdx  [+9] r10  [+10] r8  [+11] r9
    //   [+12] rbp  [+13] r12  [+14] r13  [+15] r14  [+16] r15
    // 6. Call handler
    "lea rdi, [rsp + 520]",
    "call syscall_fast_handler",
    // 7. Restore FPU/SSE before user return
    "fxrstor64 [rsp]",
    "add rsp, 520",
    // Restore user state and return via iretq
    // Save return value (rax) before CR3 restore clobbers it
    "push rax",
    // Read saved CR3 from [rsp+8] (below the push we just did)
    "mov rax, [rsp + 8]", // rax = user CR3
    "test rax, rax",
    "jz 78f",
    "mov cr3, rax",
    "78:",
    // Restore return value
    "pop rax",
    "add rsp, 16", // skip user_cr3 and saved original RBX slots
    "pop rbx",     // user RSP
    "pop rcx",     // user RIP
    "pop r11",     // user RFLAGS
    // Restore caller-saved arg registers (Go relies on r9=g surviving SYSCALL)
    // Push order was: r9, r8, r10, rdx, rsi, rdi, rax → pop in reverse
    "add rsp, 8", // skip rax (return value)
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop r10",
    "pop r8",
    "pop r9",
    "pop rbp", // callee-saved
    "pop r12",
    "pop r13",
    "pop r14",
    "pop r15",
    // Build iretq frame on the kernel stack
    // Push directly from registers (rbx=user RSP, rcx=user RIP, r11=RFLAGS)
    // Do NOT use `mov rdi, rbx` — that would clobber original rdi!
    "push 0x23", // SS (USER_DATA_SEL)
    "push rbx",  // RSP (user stack, in rbx)
    "push r11",  // RFLAGS
    "push 0x1b", // CS (USER_CODE_SEL)
    "push rcx",  // RIP (user code)
    // Restore original RBX from this syscall's own frame. After the iretq
    // frame is pushed, rsp = state_base + 96; original RBX is at base + 8.
    "mov rbx, [rsp - 88]",
    "iretq",
    // ─── Page Fault ISR stub (vector 14) ───────────────────
    // Defined in global_asm! to avoid compiler prologue uncertainty.
    // CPU pushes: error_code, RIP, CS, RFLAGS, RSP, SS (6 items, 48 bytes)
    //
    // IMPORTANT: save ALL 15 GPRs + FPU/SSE BEFORE using any register as scratch.
    // PF is an exception, not a syscall — user code expects ALL state preserved.
    // The Rust handler may use XMM registers (LLVM optimization), corrupting Go's SSE state.
    ".globl page_fault_isr_stub",
    ".type page_fault_isr_stub, @function",
    "page_fault_isr_stub:",
    // Save ALL 15 GP registers FIRST — no scratch use before this point.
    "push r15",
    "push r14",
    "push r13",
    "push r12",
    "push r11",
    "push r10",
    "push r9",
    "push r8",
    "push rdi",
    "push rsi",
    "push rdx",
    "push rcx",
    "push rbx",
    "push rbp",
    "push rax",
    // Save user CR3 (now safe to use rax as scratch)
    "mov rax, cr3",
    "push rax",
    // Switch to kernel CR3
    "mov rax, [rip + KERNEL_CR3_PHYS]",
    "cmp rax, 0",
    "je 2f",
    "mov cr3, rax",
    "2:",
    // Save FPU/SSE state (512 bytes, must be 16-byte aligned).
    // After 15 pushes (120) + CR3 push (8) = 128 bytes, RSP is 16-byte aligned
    // (IST top is page-aligned, CPU pushes 48 bytes = 16B aligned, +128 = 16B aligned).
    // sub 512 keeps 16B alignment.
    "sub rsp, 512",
    "fxsave64 [rsp]",
    // Stack layout (rsp = stack_ptr passed to handler):
    //   [rsp + 0..512]    = fxsave area (FPU/SSE state)
    //   [rsp + 512]       = user CR3
    //   [rsp + 520..640]  = 15 GPR saves (rax,rbp,rbx,rcx,rdx,rsi,rdi,r8..r15)
    //   [rsp + 640]       = error_code (CPU-pushed)
    //   [rsp + 648]       = RIP
    //   [rsp + 656]       = CS
    //   [rsp + 664]       = RFLAGS
    //   [rsp + 672]       = RSP
    //   [rsp + 680]       = SS
    "mov rdi, rsp",
    "call page_fault_handler_raw",
    // Restore FPU/SSE state
    "fxrstor64 [rsp]",
    "add rsp, 512",
    // Restore user CR3
    "pop rax",
    "cmp rax, 0",
    "je 3f",
    "mov cr3, rax",
    "3:",
    // Restore all 15 GP registers (reverse order)
    "pop rax",
    "pop rbp",
    "pop rbx",
    "pop rcx",
    "pop rdx",
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
    "add rsp, 8", // skip error_code (CPU-pushed)
    "iretq",
    // ─── #DB (Debug Exception) ISR stub — vector 1 ────────
    // Fires on hardware data watchpoint (DR0-DR3 write breakpoint).
    // Saves full context (15 GPRs + FPU) and calls debug_handler_raw.
    ".globl debug_isr_stub",
    ".type debug_isr_stub, @function",
    "debug_isr_stub:",
    // No error code for #DB
    "push r15",
    "push r14",
    "push r13",
    "push r12",
    "push r11",
    "push r10",
    "push r9",
    "push r8",
    "push rdi",
    "push rsi",
    "push rdx",
    "push rcx",
    "push rbx",
    "push rbp",
    "push rax",
    // Save user CR3
    "mov rax, cr3",
    "push rax",
    // Switch to kernel CR3
    "mov rax, [rip + KERNEL_CR3_PHYS]",
    "cmp rax, 0",
    "je 4f",
    "mov cr3, rax",
    "4:",
    // Save FPU/SSE (512 bytes)
    "sub rsp, 512",
    "fxsave64 [rsp]",
    // Stack layout:
    //   [rsp+0..512]   = fxsave
    //   [rsp+512]      = user CR3
    //   [rsp+520..640] = 15 GPRs (rax,rbp,rbx,rcx,rdx,rsi,rdi,r8..r15)
    //   [rsp+640]      = RIP (no error code for #DB)
    //   [rsp+648]      = CS
    //   [rsp+656]      = RFLAGS
    //   [rsp+664]      = RSP
    //   [rsp+672]      = SS
    "mov rdi, rsp",
    "call debug_handler_raw",
    // Restore FPU/SSE
    "fxrstor64 [rsp]",
    "add rsp, 512",
    // Restore user CR3
    "pop rax",
    "cmp rax, 0",
    "je 5f",
    "mov cr3, rax",
    "5:",
    "pop rax",
    "pop rbp",
    "pop rbx",
    "pop rcx",
    "pop rdx",
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
    "iretq",
);

// SYSCALL instruction support is now fully implemented via syscall_entry + syscall_fast_handler.
// Go binaries can use either `syscall` instruction (LSTAR) or `int 0x80` (IDT).

unsafe extern "C" {
    fn syscall_isr_stub();
    fn timer_isr_stub();
    fn syscall_entry();
    fn page_fault_isr_stub();
    fn debug_isr_stub();
}

// ─── Rust handlers called from assembly ──────────────────────

/// Handler for int 0x80 syscalls.
/// Stack layout at state_ptr (from syscall_isr_stub):
///   [0] rax (syscall number)  ← last pushed, lowest address
///   [1] rax (placeholder for return value)
///   [2] rdi
///   [3] rsi
///   [4] rdx
///   [5] r8
///   [6] r9
///   [7] r10
///   [8] r11
#[unsafe(no_mangle)]
unsafe extern "C" fn syscall_handler_impl(state_ptr: *const u64) -> u64 {
    unsafe {
        let s = state_ptr;
        let syscall_nr = *s.add(0) as usize;
        // Stack layout (from stub push order):
        //   [0] rax (syscall nr)  [1] rax (placeholder)  [2] rdi
        //   [3] rsi  [4] rdx  [5] r8  [6] r9  [7] r10  [8] r11
        let a0 = *s.add(2) as usize; // rdi
        let a1 = *s.add(3) as usize; // rsi
        let a2 = *s.add(4) as usize; // rdx
        let a3 = *s.add(7) as usize; // r10
        let a4 = *s.add(5) as usize; // r8
        let a5 = *s.add(6) as usize; // r9

        crate::syscall::dispatch(syscall_nr, [a0, a1, a2, a3, a4, a5]) as u64
    }
}

/// Handler for timer interrupt.
#[unsafe(no_mangle)]
unsafe extern "C" fn timer_trap_handler(ctx: &mut super::trap::TrapContext) {
    let stack_ptr = ctx as *mut _ as usize;
    // timer_isr_stub stack layout after fxsave:
    //   +0..512   = fxsave area
    //   +512..632 = 15 pushed GPRs
    //   +632      = interrupted RIP
    //   +640      = interrupted CS
    let interrupted_cs = unsafe { *((stack_ptr + 640) as *const u64) };
    let from_user = interrupted_cs & 0x3 != 0;
    super::lapic::local_eoi();
    crate::driver::tty::poll_uart();
    crate::arch::platform::tick_uptime();
    crate::sched::tick_sleep_queue();

    // Poll network stack (~10ms interval, same as RISC-V)
    if crate::net::iface::NetStack::is_initialized() {
        crate::net::iface::NetStack::poll();
    }

    TICK_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if !from_user {
        // Do not preempt kernel/syscall code. Returning with a user CR3 while
        // continuing kernel execution corrupts the execution context.
        let kernel_pt = crate::mm::vmm::get_kernel_page_table();
        super::trap::activate_page_table(kernel_pt as *const _ as usize);
        return;
    }

    crate::sched::schedule();
    if let Some(ksp) = crate::sched::current_kernel_stack() {
        unsafe { crate::arch::idt::SYSCALL_KSP = ksp };
    }
    let target_root = crate::process::current_page_table_root();
    if target_root != 0 {
        super::trap::activate_page_table((target_root << 12) as usize);
    } else {
        let kernel_pt = crate::mm::vmm::get_kernel_page_table();
        super::trap::activate_page_table(kernel_pt as *const _ as usize);
    }

    // ── Restore FS_BASE for the current task ──
    let fs_base = super::trap::PENDING_FS_BASE.load(core::sync::atomic::Ordering::Relaxed);
    if fs_base != 0 {
        let high = (fs_base >> 32) as u32;
        let low = fs_base as u32;
        unsafe {
            core::arch::asm!(
                "mov rcx, 0xC0000100",
                "wrmsr",
                in("edx") high,
                in("eax") low,
                out("rcx") _,
            );
        }
        super::trap::PENDING_FS_BASE.store(0, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Stub for LAPIC EOI (called from assembly).
#[unsafe(no_mangle)]
unsafe extern "C" fn lapic_local_eoi() {
    super::lapic::local_eoi();
}

/// Restore user CR3 before SYSCALL return.
/// Called from SYSCALL entry asm (inline `call restore_user_cr3`).
/// Timer ISR may have switched CR3 during syscall handling via activate_page_table().
/// This ensures iretq returns with the correct user page table active.
#[unsafe(no_mangle)]
unsafe extern "C" fn restore_user_cr3() {
    let target_root = crate::process::current_page_table_root();
    if target_root != 0 {
        let paddr = (target_root << 12) as u64;
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) paddr);
        }
    }
}

/// Handler for SYSCALL fast entry (via MSR LSTAR).
/// Stack layout at state_ptr (rsp when called):
/// The push order: r15, r14, r13, r12, rbp, r9, r8, r10, rdx, rsi, rdi,
/// rax, r11, rcx, user_rsp, original_rbx, user_cr3_placeholder.
/// rsp points to the user_cr3 placeholder.
///   [0]  user_cr3  [1] original RBX  [2] user RSP  [3] user RIP
///   [4]  user RFLAGS  [5] syscall_nr  [6] rdi  [7] rsi  [8] rdx
///   [9]  r10  [10] r8  [11] r9  [12] rbp  [13] r12  [14] r13
///   [15] r14  [16] r15
#[unsafe(no_mangle)]
unsafe extern "C" fn syscall_fast_handler(state_ptr: *const u64) -> u64 {
    // CR3 was already switched to kernel CR3 by syscall_entry assembly.
    // User CR3 is saved on the stack at [0] for the return path.
    unsafe {
        let s = state_ptr;
        let user_cr3_slot = s.add(0); // filled by assembly with actual user CR3
        let saved_rbx = *s.add(1); // original user RBX
        let user_rsp = *s.add(2); // rbx used as temporary user RSP
        let user_rip = *s.add(3); // rcx
        let user_rflags = *s.add(4); // r11
        let syscall_nr = *s.add(5); // rax
        let a0 = *s.add(6); // rdi
        let a1 = *s.add(7); // rsi
        let a2 = *s.add(8); // rdx
        let a3 = *s.add(9); // r10
        let a4 = *s.add(10); // r8
        let a5 = *s.add(11); // r9
        let saved_rbp = *s.add(12); // rbp
        let saved_r12 = *s.add(13); // r12
        let saved_r13 = *s.add(14); // r13
        let saved_r14 = *s.add(15); // r14
        let saved_r15 = *s.add(16); // r15

        // User CR3 was saved by syscall_entry assembly at stack slot [0].
        // No need to write it again — the assembly return path reads [0] directly.
        let _ = user_cr3_slot; // used by assembly return path

        // Build a TrapContext on the kernel stack so that linux_clone
        // can read the parent's full register state.
        let mut ctx = super::trap::TrapContext {
            rax: syscall_nr,
            rbx: saved_rbx, // ✅ original rbx (not user RSP)
            rcx: user_rip,
            rdx: a2,
            rbp: saved_rbp,
            rsi: a1,
            rdi: a0,
            r8: a4,
            r9: a5,
            r10: a3,
            r11: user_rflags,
            r12: saved_r12,
            r13: saved_r13,
            r14: saved_r14,
            r15: saved_r15,
            rip: user_rip,
            cs: 0x1b, // USER_CODE_SEL
            rflags: user_rflags,
            rsp: user_rsp,
            ss: 0x23, // USER_DATA_SEL
            kernel_sp: 0,
            user_cr3: 0,
            trap_from_user: 1,
        };
        crate::process::set_trap_ctx_ptr(&mut ctx as *mut _ as usize);

        let result = crate::syscall::dispatch_syscall_linux(syscall_nr, a0, a1, a2, a3, a4, a5);

        crate::process::set_trap_ctx_ptr(0);

        result
    }
}

// ─── Exception handlers ──────────────────────────────────────

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] BP at {:#x}",
        frame.instruction_pointer.as_u64()
    );
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, _error_code: u64) -> ! {
    // Switch to kernel CR3 for reliable output
    let kcr3 = crate::mm::vmm::kernel_cr3();
    if kcr3 != 0 {
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) kcr3);
        }
    }
    crate::console_println!(
        "[DF] RIP={:#x} CS={:#x} RSP={:#x} RFLAGS={:#x}",
        frame.instruction_pointer.as_u64(),
        frame.code_segment.0 as u64,
        frame.stack_pointer.as_u64(),
        frame.cpu_flags,
    );
    let cr3: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3) };
    let current = crate::sched::CURRENT_RUNNING.load(core::sync::atomic::Ordering::Relaxed);
    crate::console_println!("[DF] CR3={:#x} CURRENT_RUNNING={}", cr3, current);
    loop {}
}

extern "x86-interrupt" fn gp_fault_handler(frame: InterruptStackFrame, err: u64) {
    let from_user = frame.code_segment.0 as u64 & 0x3 != 0;
    let rip = frame.instruction_pointer.as_u64();
    let rsp = frame.stack_pointer.as_u64();
    let rflags = frame.cpu_flags.bits();
    let cs = frame.code_segment.0 as u64;
    let ss = frame.stack_segment.0 as u64;
    let cr2: u64;
    let ds: u16;
    let es: u16;
    let fs: u16;
    let gs: u16;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {0:x}, ds", out(reg) ds, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {0:x}, es", out(reg) es, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {0:x}, fs", out(reg) fs, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {0:x}, gs", out(reg) gs, options(nomem, nostack, preserves_flags));
    }
    // Print diagnostic for ALL GP faults (user and kernel)
    {
        let mut print_hex = |mut v: u64| {
            for _ in 0..16 {
                let nibble = ((v >> 60) & 0xF) as u8;
                let ch = if nibble < 10 {
                    b'0' + nibble
                } else {
                    b'a' + (nibble - 10)
                };
                crate::arch::platform::console_putchar(ch);
                v <<= 4;
            }
        };
        let mut print_str = |s: &[u8]| {
            for &ch in s {
                crate::arch::platform::console_putchar(ch);
            }
        };
        print_str(b"\n[GP] ");
        print_str(if from_user { b"USER" } else { b"KERN" });
        print_str(b" RIP=");
        print_hex(rip);
        print_str(b" RSP=");
        print_hex(rsp);
        print_str(b" ERR=");
        print_hex(err);
        print_str(b" CS=");
        print_hex(cs);
        print_str(b" SS=");
        print_hex(ss);
        print_str(b" RFLAGS=");
        print_hex(rflags);
        {
            let rbx: u64;
            unsafe { core::arch::asm!("mov {}, rbx", out(reg) rbx) };
            print_str(b" RBX=");
            print_hex(rbx);
        }
        {
            let rax: u64;
            unsafe { core::arch::asm!("mov {}, rax", out(reg) rax) };
            print_str(b" RAX=");
            print_hex(rax);
        }
        {
            let rcx: u64;
            unsafe { core::arch::asm!("mov {}, rcx", out(reg) rcx) };
            print_str(b" RCX=");
            print_hex(rcx);
        }
        {
            let rdx: u64;
            unsafe { core::arch::asm!("mov {}, rdx", out(reg) rdx) };
            print_str(b" RDX=");
            print_hex(rdx);
        }
        {
            let rsi: u64;
            unsafe { core::arch::asm!("mov {}, rsi", out(reg) rsi) };
            print_str(b" RSI=");
            print_hex(rsi);
        }
        {
            let r8: u64;
            unsafe { core::arch::asm!("mov {}, r8", out(reg) r8) };
            print_str(b" R8=");
            print_hex(r8);
        }
        print_str(b"\n");
        // Print CR3 for debugging
        {
            let cr3_val: u64;
            unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3_val) };
            print_str(b" CR3=");
            print_hex(cr3_val);
            let kc3 = crate::arch::idt::get_kernel_cr3_phys() as u64;
            print_str(b" KC3=");
            print_hex(kc3);
        }
        print_str(b"\n");
        print_str(b"[GP-SEGS] DS=");
        print_hex(ds as u64);
        print_str(b" ES=");
        print_hex(es as u64);
        print_str(b" FS=");
        print_hex(fs as u64);
        print_str(b" GS=");
        print_hex(gs as u64);
        print_str(b" CR2=");
        print_hex(cr2);
        print_str(b"\n");
    }
    // Kernel-mode GP fault during syscall handling.
    // Switch to kernel CR3 first to ensure safe memory access.
    let cr3: usize;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3) };
    let kernel_cr3 = crate::arch::idt::get_kernel_cr3_phys() & !0xFFF;
    let current_cr3 = cr3 & !0xFFF;
    let user_root = crate::process::current_page_table_root();

    if current_cr3 != kernel_cr3 {
        // Switch before any console_println!/formatting: formatting can touch
        // the kernel heap, which is not safe while still running on user CR3.
        unsafe { core::arch::asm!("mov cr3, {}", in(reg) kernel_cr3) };
        unsafe { core::arch::asm!("invlpg [{}]", in(reg) 0usize) };
    }

    crate::console_println!(
        "[GP] kernel_cr3={:#x} current_cr3={:#x} user_root={:#x}",
        kernel_cr3,
        current_cr3,
        user_root
    );
    let user_cr3_window_line =
        super::trap::USER_CR3_WINDOW_LINE.load(core::sync::atomic::Ordering::Relaxed);
    if user_cr3_window_line != 0 {
        crate::console_println!(
            "[GP-CR3-WINDOW] with_user_cr3 caller line={}",
            user_cr3_window_line
        );
    }
    if from_user && user_root != 0 {
        let user_pt = unsafe { &mut *((user_root << 12) as *mut crate::mm::vmm::PageTable) };
        // Read user stack via translated physical addresses. The GP handler is
        // running under kernel CR3 here, so direct user virtual reads can fault.
        for i in 0..8usize {
            let addr = rsp as usize + i * 8;
            if let Some(phys) = crate::mm::vmm::translate_user(user_pt, addr) {
                let val = unsafe { core::ptr::read_volatile(phys as *const usize) };
                if val > 0x100000 {
                    crate::console_println!(
                        "[GP-USTACK] [{:#x}] phys={:#x} = {:#x}",
                        addr,
                        phys,
                        val
                    );
                }
            } else {
                crate::console_println!("[GP-USTACK] [{:#x}] unmapped", addr);
            }
        }
        let rip_pte = crate::mm::vmm::debug_pte(user_pt, rip as usize);
        let rip_phys = crate::mm::vmm::translate_user(user_pt, rip as usize);
        let target = 0x465f142usize;
        let target_pte = crate::mm::vmm::debug_pte(user_pt, target);
        let target_phys = crate::mm::vmm::translate_user(user_pt, target);
        let alias = rip_phys.unwrap_or(0) & !0xfff;
        let alias_addr = alias | (rip as usize & 0xfff);
        let alias_pte = crate::mm::vmm::debug_pte(user_pt, alias_addr);
        let alias_phys = crate::mm::vmm::translate_user(user_pt, alias_addr);
        crate::console_println!(
            "[GP-PTE] rip={:#x} pte={:?} phys={:?} target={:#x} pte={:?} phys={:?} alias={:#x} pte={:?} phys={:?}",
            rip,
            rip_pte,
            rip_phys,
            target,
            target_pte,
            target_phys,
            alias_addr,
            alias_pte,
            alias_phys
        );
        if let Some(phys) = rip_phys {
            let bytes = unsafe { core::ptr::read_volatile(phys as *const [u8; 8]) };
            crate::console_println!(
                "[GP-BYTES] rip_phys={:#x} bytes={:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                phys,
                bytes[0],
                bytes[1],
                bytes[2],
                bytes[3],
                bytes[4],
                bytes[5],
                bytes[6],
                bytes[7]
            );
        }
    } else {
        // Kernel stack addresses are valid under kernel CR3.
        for i in 0..8usize {
            let addr = rsp as usize + i * 8;
            if addr > 0x100000 {
                let val = unsafe { core::ptr::read_volatile(addr as *const usize) };
                if val > 0x100000 {
                    crate::console_println!("[GP-STACK] [{:#x}] = {:#x}", addr, val);
                }
            }
        }
    }
    // Terminate the offending process
    crate::syscall::dispatch(1, [99, 0, 0, 0, 0, 0]); // exit(99)
}

// Page Fault ISR stub is now defined in global_asm! above (page_fault_isr_stub).
// This avoids compiler prologue uncertainty with non-naked functions.

/// Raw page fault handler called from global_asm! stub.
/// `stack_ptr` points to the bottom of our save area.
/// Stack layout:
///   [stack_ptr + 0..512]   fxsave area (FPU/SSE)
///   [stack_ptr + 512]      user CR3
///   [stack_ptr + 520..640] 15 GPR saves
///   [stack_ptr + 640]      error_code (CPU-pushed)
///   [stack_ptr + 648]      RIP
///   [stack_ptr + 656]      CS
///   [stack_ptr + 664]      RFLAGS
///   [stack_ptr + 672]      RSP
///   [stack_ptr + 680]      SS
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
/// #DB (Debug Exception) handler — fires on hardware write watchpoint (DR0).
/// Stack layout (from debug_isr_stub):
///   [rsp+0..512]   = fxsave
///   [rsp+512]      = user CR3
///   [rsp+520..640] = 15 GPRs (rax,rbp,rbx,rcx,rdx,rsi,rdi,r8..r15)
///   [rsp+640]      = RIP (no error code for #DB)
///   [rsp+648]      = CS
///   [rsp+656]      = RFLAGS
///   [rsp+664]      = RSP
///   [rsp+672]      = SS
unsafe extern "C" fn debug_handler_raw(stack_ptr: *const u64) {
    let rsp = stack_ptr as usize;
    // Read saved registers
    let rax = *((rsp + 520) as *const u64);
    let _rbp = *((rsp + 528) as *const u64);
    let rbx = *((rsp + 536) as *const u64);
    let rcx = *((rsp + 544) as *const u64);
    let rdx = *((rsp + 552) as *const u64);
    let rsi = *((rsp + 560) as *const u64);
    let rdi = *((rsp + 568) as *const u64);
    let r8 = *((rsp + 576) as *const u64);
    let r9 = *((rsp + 584) as *const u64);
    let r10 = *((rsp + 592) as *const u64);
    let r11 = *((rsp + 600) as *const u64);
    let r12 = *((rsp + 608) as *const u64);
    let r13 = *((rsp + 616) as *const u64);
    let r14 = *((rsp + 624) as *const u64);
    let r15 = *((rsp + 632) as *const u64);
    let rip = *((rsp + 640) as *const u64);
    let cs = *((rsp + 648) as *const u64);
    let rflags = *((rsp + 656) as *const u64);
    let user_rsp = *((rsp + 664) as *const u64);
    let ss = *((rsp + 672) as *const u64);

    // Read DR6 to identify which breakpoint fired
    let dr6: u64;
    core::arch::asm!("mov {}, dr6", out(reg) dr6);

    let from_user = cs & 0x3 != 0;

    crate::console_println!(
        "[#DB] WRITE WATCHPOINT! rip={:#x} rsp={:#x} cs={:#x} dr6={:#x} user={}",
        rip,
        user_rsp,
        cs,
        dr6,
        from_user
    );
    crate::console_println!(
        "[#DB] rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rsi={:#x} rdi={:#x}",
        rax,
        rbx,
        rcx,
        rdx,
        rsi,
        rdi
    );
    crate::console_println!(
        "[#DB] r8={:#x} r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
        r8,
        r9,
        r10,
        r11,
        r12,
        r13,
        r14,
        r15
    );
    crate::console_println!("[#DB] rflags={:#x} ss={:#x}", rflags, ss);

    // If DR6 bit 0 is set, DR0 fired
    if dr6 & 1 != 0 {
        // Read the current CR3 to identify context
        let cr3: u64;
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
        crate::console_println!("[#DB] DR0 fired! CR3={:#x}", cr3);

        // Read the watched address to show what was written
        let watched_addr = WATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
        if watched_addr != 0 {
            let val = unsafe { core::ptr::read_volatile(watched_addr as *const u64) };
            crate::console_println!(
                "[#DB] watched addr={:#x} new value={:#x}",
                watched_addr,
                val
            );
        }
    }

    // Clear DR6 (write 0 to acknowledge)
    core::arch::asm!("mov dr6, {}", in(reg) 0u64);

    // Halt if this is a kernel-mode write (we want to catch the exact instruction)
    if !from_user {
        crate::console_println!(
            "[#DB] KERNEL-MODE write to watched address! Halting for inspection."
        );
        // Don't halt — continue execution so we can see multiple hits
    }
}

/// Address being watched by DR0 hardware breakpoint.
static WATCH_ADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Set a hardware write watchpoint on `addr` using DR0.
/// After this, any 1-byte write to `addr` will trigger #DB.
pub fn set_write_watchpoint(addr: usize) {
    WATCH_ADDR.store(addr, core::sync::atomic::Ordering::Relaxed);
    unsafe {
        // DR0 = linear address to watch
        core::arch::asm!("mov dr0, {}", in(reg) addr as u64);
        // DR7 = enable DR0 write breakpoint (local)
        // Bits: [0] = L0 (local enable for DR0)
        //       [16..17] = R/W0: 01 = write only
        //       [18..19] = LEN0: 00 = 1 byte
        let dr7: u64 = 0x0001_0001; // L0=1, R/W0=write, LEN0=1byte
        core::arch::asm!("mov dr7, {}", in(reg) dr7);
    }
    crate::console_println!("[DR0] Write watchpoint set on addr={:#x}", addr);
}

/// Disable the DR0 write watchpoint.
pub fn clear_write_watchpoint() {
    unsafe {
        core::arch::asm!("mov dr7, {}", in(reg) 0u64);
        core::arch::asm!("mov dr0, {}", in(reg) 0u64);
    }
    WATCH_ADDR.store(0, core::sync::atomic::Ordering::Relaxed);
}

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
unsafe extern "C" fn page_fault_handler_raw(stack_ptr: *const u64) {
    let sp = stack_ptr as usize;
    // Read user CR3 from stub save area (at offset 512 — after fxsave, before GPRs).
    // The PF stub already switched to kernel CR3, so reading CR3 directly would give kernel CR3.
    let saved_user_cr3 = unsafe { *((sp + 512) as *const u64) };
    let raw_cr3 = saved_user_cr3;

    let fault_addr = x86_64::registers::control::Cr2::read();
    let fault_addr_val = fault_addr.map(|a| a.as_u64()).unwrap_or(0) as usize;

    // Read CPU interrupt frame from known stack offsets:
    // Stub pushes: fxsave (512) + user_cr3 (8) + 15 GPRs (120) = 640 bytes.
    // Above that: CPU-pushed error_code + iretq frame.
    let error_code = unsafe { *((sp + 640) as *const u64) };
    let rip = unsafe { *((sp + 648) as *const u64) };
    let cs = unsafe { *((sp + 656) as *const u64) };
    let rflags = unsafe { *((sp + 664) as *const u64) };
    let rsp_val = unsafe { *((sp + 672) as *const u64) };
    let ss = unsafe { *((sp + 680) as *const u64) };

    let from_user = cs & 0x3 != 0;
    let page_size = crate::mm::pmm::page_size();
    let page_addr = fault_addr_val & !(page_size - 1);

    // Determine if we should try lazy allocation for user-space addresses.
    // This applies to:
    //   1. User-mode faults (from_user = true)
    //   2. Kernel-mode faults with user CR3 (kernel accessing user memory during syscall)
    // Kernel-mode with kernel CR3 should NOT try lazy allocation — those are real kernel bugs.
    let kernel_cr3_val = unsafe { KERNEL_CR3 };
    let can_lazy_alloc = from_user || raw_cr3 != kernel_cr3_val;

    // Print concise PF info
    let mut fs_base: u64 = 0;
    unsafe {
        core::arch::asm!("rdmsr", "shl rdx, 32", "or rdx, rax", out("edx") fs_base, out("eax") _, in("ecx") 0xC0000100u32)
    };
    if !from_user {
        // Kernel PF accessing user memory — try lazy alloc (common during syscall)
        // Only print as WARNING if NOT in user address space
        if fault_addr_val < crate::process::USER_MMAP_BASE
            || fault_addr_val >= crate::process::USER_MMAP_LIMIT
        {
            crate::console_println!(
                "[PF] KERN FATAL addr={:#x} rip={:#x} cr3={:#x}",
                fault_addr_val,
                rip,
                raw_cr3
            );
        }
    } else {
        // User PF
        crate::console_println!(
            "[PF] USER addr={:#x} page={:#x} rip={:#x} rsp={:#x} err={:#x} P={} W={} U={} pid={} fs_base={:#x} cr3={:#x} can_lazy={}",
            fault_addr_val,
            page_addr,
            rip,
            rsp_val,
            error_code,
            error_code & 1,
            (error_code >> 1) & 1,
            (error_code >> 2) & 1,
            crate::process::current_pid(),
            fs_base,
            raw_cr3,
            can_lazy_alloc
        );
    }

    // Handle user-mode page faults
    let handled = 'handler: {
        // If page is present (error_code bit 0 = 1), this is a protection violation.
        // Check if VMA allows the access and upgrade PTE permissions if needed.
        if error_code & 1 != 0 {
            if can_lazy_alloc && from_user {
                // Protection fault: page exists but wrong permissions.
                // Check VMA to determine if this is ELF data or anonymous mmap.
                let vma_prot = crate::syscall::vma_query(fault_addr_val);
                let vma_allows = vma_prot.map_or(false, |p| p != 0);
                // ELF segments have prot=1 (RX, read-only text) or prot=5 (RX, read-execute).
                // Anonymous mmap regions have prot=3 (RW). PROT_NONE (0) means reserved.
                let vma_is_elf = vma_prot.map_or(false, |p| p == 1 || p == 5);
                let heap_region = fault_addr_val >= crate::process::USER_HEAP_BASE
                    && fault_addr_val < crate::process::USER_HEAP_LIMIT;
                let stack_region = fault_addr_val >= crate::process::USER_STACK_BASE
                    && fault_addr_val < crate::process::USER_STACK_TOP;

                if vma_allows || heap_region || stack_region {
                    super::trap::with_kernel_cr3(|| {
                        let user_pt = super::trap::get_user_pt_safe();
                        if let Some(frame) = crate::mm::vmm::translate_user(user_pt, page_addr) {
                            let is_identity = frame == page_addr;
                            if is_identity {
                                // Identity mapping in user space — this came from copy_kernel_mappings.
                                // For ELF regions (prot=1 or prot=5), the identity mapping IS the ELF data.
                                // For other regions (prot=3 RW, prot=0 NONE), allocate fresh zeroed frame.
                                if vma_is_elf {
                                    // ELF data: just upgrade permissions, preserve the frame
                                    crate::mm::vmm::map(
                                        user_pt,
                                        page_addr,
                                        frame,
                                        crate::mm::vmm::PTEFlags::URW,
                                    );
                                } else {
                                    // Non-ELF identity mapping: replace with fresh zeroed frame
                                    if let Some(new_frame) = crate::mm::pmm::alloc_frame() {
                                        unsafe {
                                            core::ptr::write_bytes(
                                                new_frame as *mut u8,
                                                0,
                                                page_size,
                                            );
                                        }
                                        crate::mm::vmm::map(
                                            user_pt,
                                            page_addr,
                                            new_frame,
                                            crate::mm::vmm::PTEFlags::URW,
                                        );
                                    }
                                }
                            } else {
                                // Non-identity frame: just upgrade permissions
                                crate::mm::vmm::map(
                                    user_pt,
                                    page_addr,
                                    frame,
                                    crate::mm::vmm::PTEFlags::URW,
                                );
                            }
                        }
                    });
                    break 'handler true;
                }
            }
            break 'handler false;
        }

        // ── Not-present fault: lazy allocation ──────────────────────
        // Priority: VMA > heap > stack > ELF region
        // VMA check is first because mmap regions have explicit permissions.
        // PROT_NONE VMAs explicitly refuse allocation (→ segfault).

        // 1. VMA-backed lazy allocation — highest priority, any address
        if can_lazy_alloc {
            match crate::syscall::vma_query(fault_addr_val) {
                Some(vma_prot) => {
                    if vma_prot != 0 {
                        // Valid VMA with non-NONE prot → lazy allocate
                        crate::console_println!(
                            "[PF-OK] addr={:#x} prot={:#x}",
                            fault_addr_val,
                            vma_prot
                        );
                        let pte_flags = crate::syscall::prot_to_pte_flags(vma_prot);
                        super::trap::with_kernel_cr3(|| {
                            let user_pt = super::trap::get_user_pt_safe();
                            let old_pte = crate::mm::vmm::debug_pte(user_pt, page_addr);
                            let old_frame = crate::mm::vmm::translate_user(user_pt, page_addr);
                            let needs_alloc = match old_frame {
                                None => true,
                                Some(f) => f == page_addr,
                            };
                            if needs_alloc {
                                // Check if VMA indicates ELF data (prot=1 or prot=5).
                                // If so, identity mapping IS the ELF data — don't replace.
                                let vma_is_elf = vma_prot == 1 || vma_prot == 5;
                                if vma_is_elf && old_frame.is_some() {
                                    // ELF data with identity mapping — just remap with correct perms
                                    crate::mm::vmm::map(
                                        user_pt,
                                        page_addr,
                                        old_frame.unwrap(),
                                        pte_flags,
                                    );
                                } else if let Some(frame) = crate::mm::pmm::alloc_frame() {
                                    unsafe {
                                        core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                                    }
                                    crate::mm::vmm::map(user_pt, page_addr, frame, pte_flags);
                                }
                            } else {
                                // Frame already exists (from copy_kernel_mappings identity map
                                // or previous ELF segment). Remap with correct VMA permissions.
                                // This fixes the case where a page is mapped read-only (from
                                // copy_kernel_mappings) but the VMA says PROT_READ|PROT_WRITE.
                                if let Some(old_frame) = old_frame {
                                    crate::mm::vmm::map(user_pt, page_addr, old_frame, pte_flags);
                                    crate::console_println!(
                                        "[PF-REMAP] vaddr={:#x} frame={:#x} flags={:#x} (permission upgrade)",
                                        page_addr,
                                        old_frame,
                                        pte_flags.bits()
                                    );
                                }
                            }
                        });
                        break 'handler true;
                    } else {
                        // PROT_NONE — explicitly reserved, refuse to allocate → segfault
                        break 'handler false;
                    }
                }
                None => {
                    // No VMA covers this address → fall through to heap/stack/ELF
                }
            }
        }

        // 2. Heap lazy allocation (fallback — no VMA for this address)
        if can_lazy_alloc
            && fault_addr_val >= crate::process::USER_HEAP_BASE
            && fault_addr_val < crate::process::USER_HEAP_LIMIT
        {
            super::trap::with_kernel_cr3(|| {
                let user_pt = super::trap::get_user_pt_safe();
                let needs_alloc = match crate::mm::vmm::translate_user(user_pt, page_addr) {
                    None => true,
                    Some(f) => f == page_addr,
                };
                if needs_alloc {
                    if let Some(frame) = crate::mm::pmm::alloc_frame() {
                        unsafe {
                            core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                        }
                        crate::mm::vmm::map(
                            user_pt,
                            page_addr,
                            frame,
                            crate::mm::vmm::PTEFlags::URW,
                        );
                        let new_brk = page_addr + page_size;
                        if new_brk > crate::process::current_brk() {
                            crate::process::set_current_brk(new_brk);
                        }
                    }
                }
            });
            break 'handler true;
        }

        // 3. Stack lazy allocation
        if can_lazy_alloc
            && fault_addr_val >= crate::process::USER_STACK_BASE
            && fault_addr_val < crate::process::USER_STACK_TOP
        {
            super::trap::with_kernel_cr3(|| {
                let user_pt = super::trap::get_user_pt_safe();
                if crate::mm::vmm::translate_user(user_pt, page_addr).is_none() {
                    if let Some(frame) = crate::mm::pmm::alloc_frame() {
                        unsafe {
                            core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                        }
                        crate::mm::vmm::map(
                            user_pt,
                            page_addr,
                            frame,
                            crate::mm::vmm::PTEFlags::URW,
                        );
                    }
                }
            });
            break 'handler true;
        }

        // 4. ELF-loaded region fallback (0x400000..USER_HEAP_BASE)
        if can_lazy_alloc
            && fault_addr_val >= 0x400000
            && fault_addr_val < crate::process::USER_HEAP_BASE
        {
            super::trap::with_kernel_cr3(|| {
                let user_pt = super::trap::get_user_pt_safe();
                if crate::mm::vmm::translate_user(user_pt, page_addr).is_none() {
                    if let Some(frame) = crate::mm::pmm::alloc_frame() {
                        unsafe {
                            core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                        }
                        crate::mm::vmm::map(
                            user_pt,
                            page_addr,
                            frame,
                            crate::mm::vmm::PTEFlags::URW,
                        );
                        super::trap::flush_tlb();
                    }
                }
            });
            break 'handler true;
        }

        false
    };

    #[cfg(target_arch = "x86_64")]
    crate::process::check_text_probe("pf", fault_addr_val, rip as usize);

    if !handled {
        let cr3: u64;
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) cr3);
        }
        if from_user {
            let pid = crate::process::current_pid();
            let rax = unsafe { *((sp + 520) as *const u64) };
            let rbp = unsafe { *((sp + 528) as *const u64) };
            let rbx = unsafe { *((sp + 536) as *const u64) };
            let rcx = unsafe { *((sp + 544) as *const u64) };
            let rdx = unsafe { *((sp + 552) as *const u64) };
            let rsi = unsafe { *((sp + 560) as *const u64) };
            let rdi = unsafe { *((sp + 568) as *const u64) };
            let r8 = unsafe { *((sp + 576) as *const u64) };
            let r9 = unsafe { *((sp + 584) as *const u64) };
            let r10 = unsafe { *((sp + 592) as *const u64) };
            let r11 = unsafe { *((sp + 600) as *const u64) };
            let r12 = unsafe { *((sp + 608) as *const u64) };
            let r13 = unsafe { *((sp + 616) as *const u64) };
            let r14 = unsafe { *((sp + 624) as *const u64) };
            let r15 = unsafe { *((sp + 632) as *const u64) };
            // User-mode unhandled PF — terminate the process (segfault)
            crate::console_println!(
                "[PF] UNHANDLED addr={:#x} rip={:#x} pid={} error={:#x}",
                fault_addr_val,
                rip,
                pid,
                error_code
            );
            crate::console_println!(
                "[PF] regs rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rbp={:#x} rsi={:#x} rdi={:#x}",
                rax,
                rbx,
                rcx,
                rdx,
                rbp,
                rsi,
                rdi
            );
            crate::console_println!(
                "[PF] regs r8={:#x} r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
                r8,
                r9,
                r10,
                r11,
                r12,
                r13,
                r14,
                r15
            );
            // Dump VMA state for the faulting address region
            let vma_prot = crate::syscall::vma_query(fault_addr_val);
            crate::console_println!(
                "[PF] vma_query({:#x}) = {:?}, can_lazy_alloc={}",
                fault_addr_val,
                vma_prot,
                can_lazy_alloc
            );
            // Dump nearby VMAs
            crate::syscall::vma_dump_region(fault_addr_val);
            crate::syscall::sys_exit(99);
            loop {}
        } else {
            // Kernel PF — fatal
            let (dbg_rax, dbg_rip) = unsafe { (CLONE_DBG_RAX, CLONE_DBG_RIP) };
            let mut rsp0: u64 = 0;
            unsafe {
                let rsp0_ptr = crate::arch::gdt::TSS_RSP0_ADDR;
                if rsp0_ptr != 0 {
                    rsp0 = *(rsp0_ptr as *const u64);
                }
            }
            crate::console_println!(
                "[PF] KERN FATAL fault={:#x} rip={:#x} err={:#x} CR3={:#x} RSP0={:#x}",
                fault_addr_val,
                rip,
                error_code,
                cr3,
                rsp0
            );
            loop {}
        }
    }
}

/// Naked stub for invalid opcode (#UD) handler.
/// Since SYSCALL/SYSRET is now properly configured via MSRs, the `syscall`
/// instruction no longer triggers #UD. This handler is for genuine illegal
/// instructions (e.g., SSE/AVX instructions when not supported).
/// Kills the offending user process; halts on kernel-mode #UD.
unsafe extern "C" fn invalid_opcode_isr_stub() {
    core::arch::asm!(
        // Balance compiler prologue (push rax)
        "pop rax",
        // Get the interrupt frame pointer
        // CPU pushes: RIP, CS, RFLAGS, RSP, SS (no error code for #UD)
        // Our stack has: 1 saved reg (rax) + return addr
        // Total items between rsp and CPU frame = 1
        "mov r15, rsp",
        "add r15, 1*8",       // skip 1 saved reg to reach interrupt frame

        // Check if from user mode (CS & 3)
        "mov rax, [r15 + 8]",  // CS from interrupt frame
        "test ax, 3",
        "jz 2f",               // kernel mode → halt

        // User mode: kill the process
        "mov rdi, 1",          // exit code 1
        "call {exit}",

        // Kernel mode #UD: halt
        "2:",
        "cli",
        "hlt",
        "jmp 2b",              // infinite halt loop

        exit = sym crate::syscall::sys_exit,
        options(noreturn)
    );
}

/// Keyboard ISR (IRQ1). Uses IST[3] for stack isolation.
/// Interrupt handlers must preserve all GP registers, including caller-saved
/// registers, because they can interrupt user code immediately after SYSCALL.
#[unsafe(naked)]
unsafe extern "C" fn keyboard_isr_stub() -> ! {
    core::arch::naked_asm!(
        // Save all GP registers. Rust calls below may clobber caller-saved
        // registers such as RAX, which may still hold a syscall return value.
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rbp",
        "push rbx",
        "push rax",
        // IRQs can arrive while user CR3 is active. Switch before calling Rust
        // handlers, then restore the interrupted CR3 before iretq.
        "mov rax, cr3",
        "push rax",
        "mov rax, [rip + {kernel_cr3}]",
        "cmp rax, 0",
        "je 4f",
        "mov cr3, rax",
        "4:",
        // Save FPU/SSE state
        // CPU pushes 40 + 15 GPR(120) = 160 aligned; CR3 push(8) misaligns,
        // so reserve 512 bytes plus 8 bytes of padding.
        "sub rsp, 520",
        "fxsave64 [rsp]",
        // Read scancode from PS/2 data port
        "xor rax, rax",
        "in al, 0x60",
        "mov rdi, rax",
        "call {handle}",
        // Send EOI
        "call {eoi}",
        // Restore FPU/SSE
        "fxrstor64 [rsp]",
        "add rsp, 520",
        "pop rax",
        "mov cr3, rax",
        "pop rax",
        "pop rbx",
        "pop rbp",
        "pop rcx",
        "pop rdx",
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
        "iretq",
        handle = sym crate::driver::keyboard::handle_scancode,
        eoi = sym super::lapic::local_eoi,
        kernel_cr3 = sym KERNEL_CR3_PHYS,
    );
}

/// COM1 UART ISR (IRQ4). Uses IST[4] for stack isolation.
/// Save the complete user register state; terminal responses can interrupt
/// user code right after a syscall returns, before userspace reads RAX.
#[unsafe(naked)]
unsafe extern "C" fn com1_isr_stub() -> ! {
    core::arch::naked_asm!(
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rbp",
        "push rbx",
        "push rax",
        // IRQs can arrive while user CR3 is active. Switch before calling Rust
        // handlers, then restore the interrupted CR3 before iretq.
        "mov rax, cr3",
        "push rax",
        "mov rax, [rip + {kernel_cr3}]",
        "cmp rax, 0",
        "je 4f",
        "mov cr3, rax",
        "4:",
        // Save FPU/SSE state
        // CPU pushes 40 + 15 GPR(120) = 160 aligned; CR3 push(8) misaligns,
        // so reserve 512 bytes plus 8 bytes of padding.
        "sub rsp, 520",
        "fxsave64 [rsp]",
        // Handle UART: drain ALL interrupt types to prevent interrupt storm.
        // Read IIR to identify interrupt type, then handle accordingly.
        "mov dx, 0x3FA",       // IIR (Interrupt Identification Register)
        "in al, dx",
        "test al, 1",
        "jnz 3f",              // No pending interrupt → skip
        // Check interrupt type (bits 3:1)
        "and al, 0x0E",
        "cmp al, 0x04",        // Received Data Available
        "jne 4f",
        "call {poll}",         // Handle received data
        "jmp 5f",
        "4:",
        // Other interrupt types (Line Status, Modem Status, TX empty):
        // Read the corresponding register to clear the interrupt condition.
        "mov dx, 0x3FD",       // LSR (Line Status Register)
        "in al, dx",
        "mov dx, 0x3FE",       // MSR (Modem Status Register)
        "in al, dx",
        "5:",
        "3:",
        "call {eoi}",
        // Restore FPU/SSE
        "fxrstor64 [rsp]",
        "add rsp, 520",
        "pop rax",
        "mov cr3, rax",
        "pop rax",
        "pop rbx",
        "pop rbp",
        "pop rcx",
        "pop rdx",
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
        "iretq",
        poll = sym crate::driver::tty::poll_uart,
        eoi = sym super::lapic::local_eoi,
        kernel_cr3 = sym KERNEL_CR3_PHYS,
    );
}

extern "x86-interrupt" fn spurious_handler(_frame: InterruptStackFrame) {}

// ─── IDT init ─────────────────────────────────────────────────

/// Core IDT entry write: encodes handler address, attributes, and IST index
/// into a 128-bit IDT descriptor. Both typed and raw wrappers call this.
fn write_idt_entry(ptr: *mut u64, addr: usize, attr: u64, ist_index: u16) {
    // Hardware IST: 0 = no IST (use RSP0), 1-7 = IST table[0-6].
    // ist_index=0 means "no IST" — must map to hw_ist=0, NOT 1.
    let hw_ist = if ist_index == 0 {
        0u64
    } else {
        (ist_index as u64 + 1) & 0x7
    };
    let attr_with_ist = (attr & !(0x7)) | hw_ist;

    let selector: u64 = 0x0008;
    let lo = ((addr as u64 & 0xFFFF) << 0)
        | (selector << 16)
        | (attr_with_ist << 32)
        | (((addr as u64 >> 16) & 0xFFFF) << 48);
    let hi = (addr as u64 >> 32) & 0xFFFFFFFF;
    unsafe {
        *ptr = lo;
        *ptr.add(1) = hi;
    }
}

/// Set a naked handler on a typed IDT entry (for entries with HandlerFunc).
/// Set a naked handler on any IDT entry.
/// Works for both typed `Entry<HandlerFunc>` and raw pointer access.
/// The `ist_index` parameter uses 0-based software index (0 = no IST).
fn set_naked_handler(
    entry: &mut x86_64::structures::idt::Entry<x86_64::structures::idt::HandlerFunc>,
    addr: usize,
    attr: u64,
    ist_index: u16,
) {
    write_idt_entry(entry as *mut _ as *mut u64, addr, attr, ist_index);
}

/// Set a naked handler via raw pointer (for entries with specific error code types).
/// Same as `set_naked_handler` but takes a raw `*mut u128` for flexibility.
unsafe fn set_naked_handler_raw(entry: *mut u128, addr: usize, attr: u64, ist_index: u16) {
    write_idt_entry(entry as *mut u64, addr, attr, ist_index);
}

/// Patch the IST index in an IDT entry (bits 32..34 of the low 64-bit word).
/// Uses 0-based software IST index (same as x86_64 crate's set_stack_index).
fn patch_ist_index(
    entry: &mut x86_64::structures::idt::Entry<x86_64::structures::idt::HandlerFunc>,
    ist_index: u16,
) {
    // Hardware IST: 0 = no IST, 1-7 = IST table[0-6].
    // ist_index=0 means "no IST" — must map to hw_ist=0, NOT 1.
    let hw_ist = if ist_index == 0 {
        0u64
    } else {
        ((ist_index as u64 + 1) & 0x7)
    };
    unsafe {
        let ptr = entry as *mut _ as *mut u64;
        let mut lo = *ptr;
        lo &= !(0x7 << 32); // Clear IST bits [34:32]
        lo |= hw_ist << 32; // Set new IST index (hardware value)
        *ptr = lo;
    }
}

/// Disable legacy 8259 PIC by masking all interrupts.
/// After this, only LAPIC/IOAPIC interrupts are delivered.
pub fn disable_pic() {
    unsafe {
        // Mask all interrupts on master (port 0x21) and slave (port 0xA1)
        core::arch::asm!(
            "mov al, 0xFF",
            "out 0x21, al",
            "out 0xA1, al",
            out("al") _,
        );
    }
    crate::console_println!("[pic] Legacy 8259 PIC masked");
}

pub fn init() {
    // Disable legacy 8259 PIC — we use LAPIC/IOAPIC for interrupts.
    // Without this, PIC IRQ0 (timer) fires on vector 0x08, which collides
    // with the CPU Double Fault exception vector.
    disable_pic();

    IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        idt.breakpoint.set_handler_fn(breakpoint_handler);
        // #DB (Debug Exception, vector 1): hardware write watchpoint via DR0
        set_naked_handler(
            &mut idt[1], // Vector 1 = #DB
            debug_isr_stub as *const () as usize,
            0x8E00, // 64-bit interrupt gate, DPL=0, P=1
            0,      // No IST — use task kernel stack
        );
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(super::gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.general_protection_fault
            .set_handler_fn(gp_fault_handler);
        // Page Fault: naked ISR with immediate CR3 switch
        // Use raw pointer since idt.page_fault has specific error code type
        unsafe {
            set_naked_handler_raw(
                &mut idt.page_fault as *mut _ as *mut u128,
                page_fault_isr_stub as *const () as usize,
                0x8E00,                           // 64-bit interrupt gate, DPL=0, P=1
                super::gdt::PAGE_FAULT_IST_INDEX, // Use dedicated IST
            );
        }
        // Invalid opcode (#UD): naked ISR to intercept Go's `syscall` instruction (0x0F 0x05)
        set_naked_handler(
            &mut idt[6], // Vector 6 = #UD
            invalid_opcode_isr_stub as *const () as usize,
            0x8E00, // DPL=0 (kernel only for now)
            0,      // No IST
        );

        // Timer: no IST — uses TSS RSP0 (per-task kernel stack).
        set_naked_handler(
            &mut idt[TIMER_VECTOR],
            timer_isr_stub as *const () as usize,
            0x8E00,
            0,
        );

        // Keyboard (IRQ1): naked ISR with IST[3]
        set_naked_handler(
            &mut idt[KEYBOARD_VECTOR],
            keyboard_isr_stub as *const () as usize,
            0x8E00,                         // 64-bit interrupt gate, DPL=0, P=1
            super::gdt::KEYBOARD_IST_INDEX, // Software IST index 3
        );

        // COM1 UART (IRQ4): naked ISR with IST[4]
        set_naked_handler(
            &mut idt[COM1_VECTOR],
            com1_isr_stub as *const () as usize,
            0x8E00,
            super::gdt::COM1_IST_INDEX, // Software IST index 4
        );

        idt[SPURIOUS_VECTOR].set_handler_fn(spurious_handler);

        // Syscall (int 0x80): DPL=3, NO IST — use per-task kernel stack (TSS.RSP0).
        //
        // CRITICAL: We must NOT use IST for int $0x80. IST is a fixed stack that
        // is the same for every invocation. If two user tasks enter through this
        // gate, their saved frames would reuse the same IST top. Switching back
        // to an older task would then restore overwritten garbage → Double Fault.
        //
        // Using IST index 0 (= no IST) makes the CPU use TSS.RSP0 instead, which
        // is per-task (updated by schedule() and gdt::set_kernel_rsp0()). Each
        // task's TrapContext lives on its own kernel stack, safely isolated.
        set_naked_handler(
            &mut idt[SYSCALL_VECTOR],
            syscall_isr_stub as *const () as usize,
            0xEE00, // Base attributes (DPL=3, 64-bit interrupt gate, P=1)
            0,      // IST index 0 = no IST, use TSS.RSP0 (per-task kernel stack)
        );

        idt
    });

    IDT.get().unwrap().load();
    cache_kernel_cr3();
    init_syscall_msrs();
}

pub fn load() {
    if let Some(idt) = IDT.get() {
        idt.load();
    }
    cache_kernel_cr3();
    init_syscall_msrs();
}

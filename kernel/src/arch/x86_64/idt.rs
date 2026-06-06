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
const MSR_STAR: u32 = 0xC000_281;
const MSR_LSTAR: u32 = 0xC000_282;
const MSR_CSTAR: u32 = 0xC000_283;
const MSR_SFMASK: u32 = 0xC000_284;

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
#[unsafe(no_mangle)]
static mut KERNEL_CR3: u64 = 0;

/// Kernel stack pointer for SYSCALL fast entry.
#[unsafe(no_mangle)]
static mut SYSCALL_KSP: u64 = 0;

/// Kernel CR3 physical address, used by Timer ISR for CR3 switching.
#[used]
#[cfg_attr(target_arch = "x86_64", unsafe(link_section = ".data"))]
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
static mut KERNEL_CR3_PHYS: u64 = 0;

pub fn set_syscall_ksp(ksp: u64) {
    unsafe {
        SYSCALL_KSP = ksp;
        KERNEL_CR3_PHYS = crate::mm::vmm::kernel_cr3();
    }
}

pub fn cache_kernel_cr3() {
    unsafe {
        KERNEL_CR3 = crate::mm::vmm::kernel_cr3();
    }
}

pub fn init_syscall_msrs() {
    unsafe {
        // Enable SYSCALL/SYSRET in EFER (bit 0 = SCE)
        let efer = rdmsr(MSR_EFER);
        wrmsr(MSR_EFER, efer | 1);
        // Set up SYSCALL MSRs
        wrmsr(MSR_STAR, (0x18u64 << 48) | (0x08u64 << 32));
        wrmsr(MSR_LSTAR, syscall_entry as usize as u64);
        wrmsr(MSR_CSTAR, 0);
        wrmsr(MSR_SFMASK, 1 << 9); // Mask IF on syscall entry
    }
}

// ─── ISR stubs defined via global_asm! ─────────────────────────
// We use global_asm! because naked_asm! doesn't support `sym` references
// reliably on nightly. global_asm! with `sym` works correctly.

core::arch::global_asm!(
    ".section .text",
    // ─── int 0x80 syscall ISR stub ───────────────────────
    ".globl syscall_isr_stub",
    ".type syscall_isr_stub, @function",
    "syscall_isr_stub:",
    "cli",
    // Save registers (9 slots, 72 bytes)
    // Stack layout from rsp (growing down):
    //   [0] = rax (syscall nr)   — last push
    //   [1] = rax (placeholder for return value)
    //   [2] = rdi (a0)
    //   [3] = rsi (a1)
    //   [4] = rdx (a2)
    //   [5] = r8  (a4)
    //   [6] = r9  (a5)
    //   [7] = r10 (a3)
    //   [8] = r11                  — first push
    "push r11",
    "push r10",
    "push r9",
    "push r8",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rax", // [1] placeholder for return value
    "push rax", // [0] syscall number
    // IMPORTANT: Do NOT enable interrupts (sti) here!
    // The Timer ISR shares the RSP0 kernel stack (via IST). If it fires
    // between sti and the call below, its pushes will clobber our
    // saved registers on the stack, corrupting syscall arguments.
    // Instead, iretq will restore IF from the user-mode RFLAGS on
    // the CPU-pushed iretq frame (user has IF=1), re-enabling
    // interrupts when we return to Ring 3.
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
    "iretq",
    // ─── Timer ISR stub ──────────────────────────────────
    ".globl timer_isr_stub",
    ".type timer_isr_stub, @function",
    "timer_isr_stub:",
    // Save all 15 GP registers
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
    // ── Switch to kernel CR3 ──
    // Timer ISR fires under user CR3 when running user programs.
    // Switch to kernel CR3 to ensure all kernel data is accessible.
    "mov rax, cr3",
    "push rax", // save user CR3 on stack
    "lea rax, [rip + KERNEL_CR3_PHYS]",
    "mov rax, [rax]", // load kernel CR3 value
    "cmp rax, 0",
    "je 7f",
    "mov cr3, rax", // switch to kernel page table
    "7:",
    "mov rdi, rsp",
    "call timer_trap_handler",
    // ── Restore user CR3 ──
    "pop rax",
    "mov cr3, rax",
    // EOI is already sent inside timer_trap_handler (before schedule/sti)
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
    // CR3 was already set by timer_trap_handler to the new task's user PT
    "iretq",
    // ─── SYSCALL fast entry (MSR LSTAR) ──────────────────
    ".globl syscall_entry",
    ".type syscall_entry, @function",
    "syscall_entry:",
    // On entry: rcx=return RIP, r11=return RFLAGS (set by CPU)
    //           rax=syscall_nr, rdi/rsi/rdx/r10/r8/r9=args
    //           rsp=user RSP (NOT switched by SYSCALL), Ring 0

    // 1. Save user RSP in callee-saved rbx
    "mov rbx, rsp",
    // 2. Switch to kernel stack
    "lea rsp, [rip + SYSCALL_KSP]",
    "mov rsp, [rsp]",
    // 3. Save user CR3 and switch to kernel CR3
    "mov rax, cr3", // rax = user CR3
    "push rax",     // [rsp+0x50] user CR3 (on kernel stack)
    "lea rax, [rip + KERNEL_CR3]",
    "mov rax, [rax]", // rax = kernel CR3
    "mov cr3, rax",   // switch to kernel page table
    // 4. Build register state frame on kernel stack
    "push rbx", // [rsp+0x48] user RSP
    "push r11", // [rsp+0x40] user RFLAGS
    "push rcx", // [rsp+0x38] user RIP
    "push r9",  // [rsp+0x30] a5
    "push r8",  // [rsp+0x28] a4
    "push r10", // [rsp+0x20] a3
    "push rdx", // [rsp+0x18] a2
    "push rsi", // [rsp+0x10] a1
    "push rdi", // [rsp+0x08] a0
    "push rax", // [rsp+0x00] syscall_nr
    // 5. Call Rust handler
    "mov rdi, rsp",
    "call syscall_fast_handler",
    // 6. Return path: rax = return value
    //    Skip syscall_nr, a0..a5 (8 slots)
    "add rsp, 8*8",
    // Restore user RIP and RFLAGS
    "pop rcx", // user RIP → rcx (for sysretq)
    "pop r11", // user RFLAGS → r11 (for sysretq)
    // Restore user RSP
    "pop rsp", // user RSP → rsp
    // 7. Restore user CR3
    "pop rax", // user CR3
    "mov cr3, rax",
    // 8. Return to user: rcx→RIP, r11→RFLAGS (CPU does this)
    "sysretq",
);

// SYSCALL instruction support is now fully implemented via syscall_entry + syscall_fast_handler.
// Go binaries can use either `syscall` instruction (LSTAR) or `int 0x80` (IDT).

unsafe extern "C" {
    fn syscall_isr_stub();
    fn timer_isr_stub();
    fn syscall_entry();
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
    let _ = ctx;

    // Send EOI early — before any 'sti' that might allow nested interrupts.
    // If we don't EOI before schedule(), the LAPIC will see the timer vector
    // as still in-service and may deliver another timer interrupt on 'sti',
    // causing nested Timer ISR on the same IST stack.
    super::lapic::local_eoi();

    crate::driver::tty::poll_uart();
    crate::arch::platform::tick_uptime();
    super::lapic::set_next_timer();
    crate::sched::schedule();

    // After schedule(), activate the new task's page table
    let target_root = crate::process::current_page_table_root();
    if target_root != 0 {
        super::trap::activate_page_table((target_root << 12) as usize);
    }

    let ksp = crate::sched::current_kernel_sp();
    if ksp != 0 {
        set_syscall_ksp(ksp as u64);
    }
}

/// Stub for LAPIC EOI (called from assembly).
#[unsafe(no_mangle)]
unsafe extern "C" fn lapic_local_eoi() {
    super::lapic::local_eoi();
}

/// Handler for SYSCALL fast entry (via MSR LSTAR).
/// Stack layout at state_ptr (from syscall_entry):
///   [0] rax (syscall number)
///   [1] rdi (a0)
///   [2] rsi (a1)
///   [3] rdx (a2)
///   [4] r10 (a3)
///   [5] r8  (a4)
///   [6] r9  (a5)
///   [7] rcx (user RIP)
///   [8] r11 (user RFLAGS)
///   [9] user RSP
///   [10] user CR3
#[unsafe(no_mangle)]
unsafe extern "C" fn syscall_fast_handler(state_ptr: *const u64) -> u64 {
    unsafe {
        let s = state_ptr;
        let syscall_nr = *s.add(0) as usize;
        let a0 = *s.add(1) as usize;
        let a1 = *s.add(2) as usize;
        let a2 = *s.add(3) as usize;
        let a3 = *s.add(4) as usize;
        let a4 = *s.add(5) as usize;
        let a5 = *s.add(6) as usize;

        crate::syscall::dispatch(syscall_nr, [a0, a1, a2, a3, a4, a5]) as u64
    }
}

// ─── Exception handlers ──────────────────────────────────────

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] BP at {:#x}",
        frame.instruction_pointer.as_u64()
    );
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, error_code: u64) -> ! {
    // Switch to kernel CR3 for reliable output
    let kcr3 = crate::mm::vmm::kernel_cr3();
    if kcr3 != 0 {
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) kcr3);
        }
    }
    // WARNING: The InterruptStackFrame fields are shifted by 8 bytes relative
    // to the actual CPU push order. The `error_code` parameter contains the
    // actual RIP of the faulting context.
    // Real layout:  [SS, RSP, RFLAGS, CS, RIP] ← pushed by CPU
    // What we get:  error_code=RIP, instruction_pointer=CS, code_segment=RFLAGS,
    //                cpu_flags=RSP, stack_pointer=SS
    let actual_rip = error_code;
    let actual_cs = frame.instruction_pointer.as_u64();
    let actual_rflags = frame.code_segment.0 as u64;
    let actual_rsp = frame.cpu_flags;
    let actual_ss = frame.stack_pointer.as_u64();
    crate::console_println!(
        "[DF] RIP={:#x} CS={:#x} RSP={:#x} SS={:#x} RFLAGS={:#x}",
        actual_rip,
        actual_cs,
        actual_rsp,
        actual_ss,
        actual_rflags
    );
    loop {}
}

extern "x86-interrupt" fn gp_fault_handler(frame: InterruptStackFrame, err: u64) {
    let from_user = frame.code_segment.0 as u64 & 0x3 != 0;
    crate::console_println!(
        "[EXCEPTION] GP Fault at {:#x}, err={:#x}, from_user={}",
        frame.instruction_pointer.as_u64(),
        err,
        from_user
    );
    if from_user {
        crate::syscall::dispatch(1, [1, 0, 0, 0, 0, 0]);
    } else {
        loop {}
    }
}

/// Naked stub for Page Fault handler (vector 14).
/// Switches to kernel CR3 immediately because the PF handler must access
/// kernel data structures that may not be mapped in the user page table.
/// Without this, a PF from Ring 3 → Ring 0 runs under user CR3 →
/// accessing kernel data → nested PF → Double Fault.
#[cfg(target_arch = "x86_64")]
unsafe extern "C" fn page_fault_isr_stub() {
    core::arch::asm!(
        // Balance compiler prologue (push rax). Since this is not a naked
        // function, the compiler emits `push rax` before our asm! block.
        "pop rax",
        // CPU has already pushed: error_code, RIP, CS, RFLAGS, RSP, SS
        // Save all GP registers
        "push rax",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        // Don't need to save callee-saved (rbx, rbp, r12-r15) since
        // page_fault_handler_body doesn't modify them

        // ── Switch to kernel CR3 ──
        "mov rax, cr3",
        "push rax",                    // save user CR3 on stack
        "mov rax, {kcr3}",
        "cmp rax, 0",
        "je 5f",
        "mov cr3, rax",                // switch to kernel page table
        "5:",

        // Call Rust handler body
        // RDI = pointer to InterruptStackFrame (skip our saved regs)
        // RSI = error code (at RSP + 10*8 from our pushes + saved CR3)
        "mov rdi, rsp",
        "add rdi, 11*8",               // skip 10 saved regs + saved CR3 → points to error_code
        "mov rsi, [rdi]",              // error code
        "add rdi, 8",                  // skip error code → InterruptStackFrame
        "call {body}",

        // ── Restore user CR3 ──
        "pop rax",
        "mov cr3, rax",

        // Restore GP registers
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "add rsp, 8",                  // skip error code
        "iretq",
        kcr3 = sym crate::mm::vmm::kernel_cr3,
        body = sym page_fault_handler_body,
        options(noreturn)
    );
}

/// Page fault handler body (called from naked stub with kernel CR3).
#[cfg(target_arch = "x86_64")]
fn page_fault_handler_body(frame: &InterruptStackFrame, _error_code: u64) {
    // CR3 already switched to kernel by naked stub.
    let fault_addr = x86_64::registers::control::Cr2::read();
    let fault_addr_val = fault_addr.map(|a| a.as_u64()).unwrap_or(0) as usize;
    let from_user = frame.code_segment.0 as u64 & 0x3 != 0;
    let page_size = crate::mm::pmm::page_size();
    let page_addr = fault_addr_val & !(page_size - 1);

    // Debug: log page fault
    crate::console_println!(
        "[PF] addr={:#x} page={:#x} user={} ip={:#x}",
        fault_addr_val,
        page_addr,
        from_user,
        frame.instruction_pointer.as_u64()
    );

    let handled = 'handler: {
        // Try lazy allocation for heap
        if from_user
            && fault_addr_val >= crate::process::USER_HEAP_BASE
            && fault_addr_val < crate::process::USER_HEAP_LIMIT
        {
            let user_pt = super::trap::get_current_user_pt();
            if crate::mm::vmm::translate_user(user_pt, page_addr).is_none() {
                if let Some(frame) = crate::mm::pmm::alloc_frame() {
                    unsafe {
                        core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                    }
                    crate::mm::vmm::map(user_pt, page_addr, frame, crate::mm::vmm::PTEFlags::URW);
                    super::trap::flush_tlb_addr(page_addr);
                    let new_brk = page_addr + page_size;
                    if new_brk > crate::process::current_brk() {
                        crate::process::set_current_brk(new_brk);
                    }
                    break 'handler true;
                }
            }
            break 'handler true;
        }

        // Try lazy allocation for stack
        if from_user
            && fault_addr_val >= crate::process::USER_STACK_BASE
            && fault_addr_val < crate::process::USER_STACK_TOP
        {
            let user_pt = super::trap::get_current_user_pt();
            if crate::mm::vmm::translate_user(user_pt, page_addr).is_none() {
                if let Some(frame) = crate::mm::pmm::alloc_frame() {
                    unsafe {
                        core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                    }
                    crate::mm::vmm::map(user_pt, page_addr, frame, crate::mm::vmm::PTEFlags::URW);
                    super::trap::flush_tlb_addr(page_addr);
                    break 'handler true;
                }
            }
            break 'handler true;
        }

        // Try lazy allocation for mmap region
        if from_user
            && fault_addr_val >= crate::process::USER_MMAP_BASE
            && fault_addr_val < crate::process::USER_MMAP_LIMIT
        {
            let user_pt = super::trap::get_current_user_pt();
            if crate::mm::vmm::translate_user(user_pt, page_addr).is_none() {
                if let Some(frame) = crate::mm::pmm::alloc_frame() {
                    unsafe {
                        core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                    }
                    crate::mm::vmm::map(user_pt, page_addr, frame, crate::mm::vmm::PTEFlags::URW);
                    super::trap::flush_tlb_addr(page_addr);
                    break 'handler true;
                }
            }
            break 'handler true;
        }

        false
    };

    if !handled {
        crate::console_println!(
            "[EXCEPTION] Page Fault at {:#x}, accessing {:#x}",
            frame.instruction_pointer.as_u64(),
            fault_addr_val
        );
        if from_user {
            crate::syscall::dispatch(1, [1, 0, 0, 0, 0, 0]);
        } else {
            loop {}
        }
    }
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] Invalid Opcode at {:#x}",
        frame.instruction_pointer.as_u64()
    );
}

/// Naked stub for keyboard ISR (IRQ1). Uses IST[3] for stack isolation.
/// Written as naked function because `extern "x86-interrupt"` with `patch_ist_index`
/// can cause stack corruption on `iretq`.
unsafe extern "C" fn keyboard_isr_stub() {
    core::arch::asm!(
        // Balance compiler prologue (push rax). Since this is not a naked
        // function, the compiler emits `push rax` before our asm! block.
        "pop rax",
        // Save callee-saved registers
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push rbp",
        "push rbx",
        // Read scancode from PS/2 data port
        "xor rax, rax",
        "in al, 0x60",
        "mov rdi, rax",
        "call {handle}",
        // Send EOI
        "call {eoi}",
        // Restore and return
        "pop rbx",
        "pop rbp",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "iretq",
        handle = sym crate::driver::keyboard::handle_scancode,
        eoi = sym super::lapic::local_eoi,
        options(noreturn)
    );
}

/// Naked stub for COM1 UART ISR (IRQ4). Uses IST[4] for stack isolation.
unsafe extern "C" fn com1_isr_stub() {
    core::arch::asm!(
        // Balance compiler prologue (push rax). Since this is not a naked
        // function, the compiler emits `push rax` before our asm! block.
        "pop rax",
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push rbp",
        "push rbx",
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
        "pop rbx",
        "pop rbp",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "iretq",
        poll = sym crate::driver::tty::poll_uart,
        eoi = sym super::lapic::local_eoi,
        options(noreturn)
    );
}

extern "x86-interrupt" fn spurious_handler(_frame: InterruptStackFrame) {}

// ─── IDT init ─────────────────────────────────────────────────

fn set_naked_handler(
    entry: &mut x86_64::structures::idt::Entry<x86_64::structures::idt::HandlerFunc>,
    addr: usize,
    attr: u64,
    ist_index: u16,
) {
    // Hardware IST: 0 = no IST, 1-7 = table[0-6].
    // Software IST (0-based, same as x86_64 crate's set_stack_index): 0-6.
    // Convert: hardware = software + 1.
    let hw_ist = (ist_index + 1) & 0x7;
    let attr_with_ist = (attr & !(0x7)) | hw_ist as u64;

    let selector: u64 = 0x0008;
    let lo = ((addr as u64 & 0xFFFF) << 0)
        | (selector << 16)
        | (attr_with_ist << 32)
        | (((addr as u64 >> 16) & 0xFFFF) << 48);
    let hi = (addr as u64 >> 32) & 0xFFFFFFFF;
    unsafe {
        let ptr = entry as *mut _ as *mut u64;
        *ptr = lo;
        *ptr.add(1) = hi;
    }
}

/// Same as set_naked_handler but for IDT entries that push an error code
/// (e.g., Page Fault #PF, Double Fault #DF, General Protection #GP).
/// Set a naked handler on any IDT entry (raw pointer version).
/// Works for both error-code and non-error-code entries since the
/// IDT entry format is identical — only the handler code differs.
unsafe fn set_naked_handler_raw(
    entry: *mut u128, // raw pointer to IDT entry (128 bits)
    addr: usize,
    attr: u64,
    ist_index: u16,
) {
    let hw_ist = (ist_index + 1) & 0x7;
    let attr_with_ist = (attr & !(0x7)) | hw_ist as u64;

    let selector: u64 = 0x0008;
    let lo = ((addr as u64 & 0xFFFF) << 0)
        | (selector << 16)
        | (attr_with_ist << 32)
        | (((addr as u64 >> 16) & 0xFFFF) << 48);
    let hi = (addr as u64 >> 32) & 0xFFFFFFFF;
    unsafe {
        let ptr = entry as *mut u64;
        *ptr = lo;
        *ptr.add(1) = hi;
    }
}

/// Patch the IST index in an IDT entry (bits 32..34 of the low 64-bit word).
/// Uses 0-based software IST index (same as x86_64 crate's set_stack_index).
fn patch_ist_index(
    entry: &mut x86_64::structures::idt::Entry<x86_64::structures::idt::HandlerFunc>,
    ist_index: u16,
) {
    // Convert software index (0-based) to hardware IST value (1-based).
    let hw_ist = ((ist_index + 1) & 0x7) as u64;
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
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);

        // Timer: naked ISR with CR3 switch, using software IST index 2
        // (hardware IST=3 → TSS interrupt_stack_table[2])
        set_naked_handler(
            &mut idt[TIMER_VECTOR],
            timer_isr_stub as *const () as usize,
            0x8E00,                      // Base attributes (64-bit interrupt gate, DPL=0, P=1)
            super::gdt::TIMER_IST_INDEX, // Software IST index 2
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

        // Syscall (int 0x80): DPL=3, using software IST index 1
        // (hardware IST=2 → TSS interrupt_stack_table[1])
        set_naked_handler(
            &mut idt[SYSCALL_VECTOR],
            syscall_isr_stub as *const () as usize,
            0xEE00, // Base attributes (DPL=3, 64-bit interrupt gate, P=1)
            super::gdt::SYSCALL_IST_INDEX, // Software IST index 1
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

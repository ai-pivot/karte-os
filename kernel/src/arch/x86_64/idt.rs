//! IDT setup using the `x86_64` crate.
//!
//! CPU exceptions and hardware interrupts use `extern "x86-interrupt"` handlers.
//! Syscall via `int 0x80` uses a custom naked ISR stub for full register control.
//! Go binaries use the `syscall` instruction (0x0F 0x05) which enters via MSR LSTAR.

use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

static IDT: Once<InterruptDescriptorTable> = Once::new();

pub const IRQ_BASE: u8 = 32;
pub const SYSCALL_VECTOR: u8 = 0x80;
pub const TIMER_VECTOR: u8 = IRQ_BASE + 0;
pub const KEYBOARD_VECTOR: u8 = IRQ_BASE + 1;
pub const COM1_VECTOR: u8 = IRQ_BASE + 4;
pub const SPURIOUS_VECTOR: u8 = IRQ_BASE + 7;

// ─── MSR constants for SYSCALL/SYSRET ─────────────────────────
const MSR_STAR: u32 = 0xC000_281;
const MSR_LSTAR: u32 = 0xC000_282;
const MSR_CSTAR: u32 = 0xC000_283;
const MSR_SFMASK: u32 = 0xC000_284;

/// Read a 64-bit MSR.
unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi);
    ((hi as u64) << 32) | (lo as u64)
}

/// Write a 64-bit MSR.
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!("wrmsr", in("ecx") msr, in("eax") lo, in("edx") hi);
}

/// Kernel stack pointer for the syscall fast entry path.
/// Updated by the scheduler on task switch. Single-core safe.
static mut SYSCALL_KSP: u64 = 0;

/// Update the kernel stack pointer used by the syscall fast entry path.
/// Called by the scheduler after switching tasks.
pub fn set_syscall_ksp(ksp: u64) {
    unsafe {
        SYSCALL_KSP = ksp;
    }
}

/// Configure the SYSCALL/SYSRET instruction entry point and segment selectors.
///
/// Must be called after GDT initialization.
pub fn init_syscall_msrs() {
    unsafe {
        let kernel_cs: u64 = 0x08; // GDT kernel code segment selector
        let user_cs: u64 = 0x18; // GDT user code segment selector

        // STAR[63:48] = CS for SYSRET (user), STAR[47:32] = CS for SYSCALL (kernel)
        let star = (user_cs << 48) | (kernel_cs << 32);
        wrmsr(MSR_STAR, star);

        // LSTAR = RIP of syscall entry point
        wrmsr(MSR_LSTAR, syscall_fast_entry as usize as u64);

        // CSTAR = unused (compat mode)
        wrmsr(MSR_CSTAR, 0);

        // SFMASK: clear IF (bit 9) on SYSCALL entry → interrupts disabled
        wrmsr(MSR_SFMASK, 1 << 9);
    }
}

// ─── SYSCALL fast entry point ─────────────────────────────────
//
// On SYSCALL entry (Ring 0):
//   RCX = return RIP, R11 = return RFLAGS
//   RAX = syscall number
//   RDI, RSI, RDX, R10, R8, R9 = arguments
//   RSP = user RSP (unchanged by CPU)
//   RFLAGS: IF cleared by SFMASK
//
// SYSRET return:
//   RCX → RIP, R11 → RFLAGS, RAX = return value, Ring 3

/// Naked entry point for the SYSCALL instruction.
///
/// Stack frame on kernel stack (grows down from ksp):
///   [rsp+0x48] user RSP   (for restoring after syscall)
///   [rsp+0x40] r11        (user RFLAGS, needed for sysretq)
///   [rsp+0x38] rcx        (user RIP, needed for sysretq)
///   [rsp+0x30] r9         (arg5)
///   [rsp+0x28] r8         (arg4)
///   [rsp+0x20] r10        (arg3)
///   [rsp+0x18] rdx        (arg2)
///   [rsp+0x10] rsi        (arg1)
///   [rsp+0x08] rdi        (arg0)
///   [rsp+0x00] rax        (syscall number)
#[unsafe(naked)]
unsafe extern "C" fn syscall_fast_entry() {
    unsafe {
        core::arch::naked_asm!(
            // ── Switch to kernel stack ──
            // Save user RSP in a scratch register (we use rbx, which is callee-saved
            // and not a syscall argument). We'll restore it before sysretq.
            "mov rbx, rsp",
            "mov rsp, [rip + {ksp}]",

            // ── Save context on kernel stack ──
            "push rbx",    // user RSP (at [rsp+0x48] after all pushes)
            "push r11",    // user RFLAGS
            "push rcx",    // user RIP
            "push r9",     // a5
            "push r8",     // a4
            "push r10",    // a3
            "push rdx",    // a2
            "push rsi",    // a1
            "push rdi",    // a0
            "push rax",    // syscall_nr

            // ── Call Rust handler ──
            "mov rdi, rsp",
            "call {handler}",

            // ── Restore and return via SYSRETQ ──
            // rax = return value from handler
            // Pop syscall_nr (discard), then extract args we need
            "add rsp, 8",              // skip saved rax
            "add rsp, 8*7",            // skip rdi,rsi,rdx,r10,r8,r9
            // Now rsp points to: rcx(user RIP), r11(user RFLAGS), rbx(user RSP)
            "pop rcx",                 // restore user RIP (needed for sysretq)
            "pop r11",                 // restore user RFLAGS (needed for sysretq)
            "pop rsp",                 // restore user RSP

            // rax already has the return value
            "sysretq",

            ksp = sym SYSCALL_KSP,
            handler = sym syscall_entry_handler,
        );
    }
}

/// Rust handler called from the syscall fast entry point.
///
/// Stack layout at state_ptr:
///   [0] rax (syscall_nr)
///   [1] rdi (a0)
///   [2] rsi (a1)
///   [3] rdx (a2)
///   [4] r10 (a3)
///   [5] r8  (a4)
///   [6] r9  (a5)
///   [7] rcx (user RIP)
///   [8] r11 (user RFLAGS)
///   [9] user RSP
unsafe extern "C" fn syscall_entry_handler(state_ptr: *const u64) -> u64 {
    unsafe {
        let s = state_ptr;
        let syscall_nr = *s.add(0) as usize;
        let a0 = *s.add(1) as usize;
        let a1 = *s.add(2) as usize;
        let a2 = *s.add(3) as usize;
        let a3 = *s.add(4) as usize;
        let a4 = *s.add(5) as usize;
        let a5 = *s.add(6) as usize;

        let args = [a0, a1, a2, a3, a4, a5];
        let result = crate::syscall::dispatch(syscall_nr, args);
        result as u64
    }
}

/// Build and load the IDT. Called once by BSP during boot.
pub fn init() {
    IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        // CPU exceptions
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(super::gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.general_protection_fault
            .set_handler_fn(gp_fault_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);

        // Hardware interrupts
        // Timer uses naked ISR (needs full TrapContext save for preemptive scheduling).
        // Manual IDT entry construction, same as syscall stub.
        {
            let handler_addr = timer_handler as usize as u64;
            let entry_ptr = &mut idt[TIMER_VECTOR] as *mut _ as *mut u64;
            let selector: u64 = 0x0008;
            let attr: u64 = 0x8E00; // Present | DPL0 | Interrupt Gate 64-bit | IST=0
            let lo = ((handler_addr & 0xFFFF) << 0)
                | (selector << 16)
                | (attr << 32)
                | (((handler_addr >> 16) & 0xFFFF) << 48);
            let hi = (handler_addr >> 32) & 0xFFFFFFFF;
            unsafe {
                *entry_ptr = lo;
                *entry_ptr.add(1) = hi;
            }
        }
        idt[KEYBOARD_VECTOR].set_handler_fn(keyboard_handler);
        idt[COM1_VECTOR].set_handler_fn(com1_handler);
        idt[SPURIOUS_VECTOR].set_handler_fn(spurious_handler);

        // Syscall via int 0x80 — custom naked stub for full register control
        // We cannot use set_handler_fn with a naked function directly.
        // Instead, we construct the IDT entry manually.
        {
            let handler_addr = syscall_isr_stub as usize as u64;
            let entry_ptr = &mut idt[SYSCALL_VECTOR] as *mut _ as *mut u64;
            let selector: u64 = 0x0008; // kernel code segment
            let attr: u64 = 0xEE00; // Present | DPL3 | Interrupt Gate 64-bit | IST=0
            let lo = ((handler_addr & 0xFFFF) << 0)
                | (selector << 16)
                | (attr << 32)
                | (((handler_addr >> 16) & 0xFFFF) << 48);
            let hi = (handler_addr >> 32) & 0xFFFFFFFF;
            unsafe {
                *entry_ptr = lo;
                *entry_ptr.add(1) = hi;
            }
        }

        idt
    });

    IDT.get().unwrap().load();

    // Initialize SYSCALL/SYSRET MSRs for Go binary support
    init_syscall_msrs();
}

/// Load the already-built IDT. Called by APs to load the shared IDT.
pub fn load() {
    if let Some(idt) = IDT.get() {
        idt.load();
    }
    // APs also need syscall MSRs
    init_syscall_msrs();
}

// ─── CPU Exceptions ──────────────────────────────────────────

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] BP at {:#x}",
        frame.instruction_pointer.as_u64()
    );
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, _err: u64) -> ! {
    panic!(
        "[EXCEPTION] Double Fault at {:#x}",
        frame.instruction_pointer.as_u64()
    );
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
        crate::console_println!("[trap] Killing user process");
        crate::syscall::dispatch(1, [1, 0, 0, 0, 0, 0]); // sys_exit(1)
    } else {
        loop {}
    }
}

extern "x86-interrupt" fn page_fault_handler(frame: InterruptStackFrame, _err: PageFaultErrorCode) {
    let fault_addr = x86_64::registers::control::Cr2::read();
    let fault_addr_val = fault_addr.map(|a| a.as_u64()).unwrap_or(0) as usize;
    let cr3 = unsafe {
        x86_64::registers::control::Cr3::read()
            .0
            .start_address()
            .as_u64()
    };

    let from_user = frame.code_segment.0 as u64 & 0x3 != 0;
    let page_size = crate::mm::pmm::page_size();
    let page_addr = fault_addr_val & !(page_size - 1);

    // Expanded lazy page allocation: heap, stack, and mmap regions
    let heap_base = crate::process::USER_HEAP_BASE;
    let heap_limit = crate::process::USER_HEAP_LIMIT;
    let stack_base = crate::process::USER_STACK_BASE;
    let stack_top = crate::process::USER_STACK_TOP;
    let mmap_base = crate::process::USER_MMAP_BASE;
    let mmap_limit = crate::process::USER_MMAP_LIMIT;

    // Try lazy allocation for heap
    if from_user && fault_addr_val >= heap_base && fault_addr_val < heap_limit {
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
                return;
            }
        }
        return;
    }

    // Try lazy allocation for stack (stack grows down)
    if from_user && fault_addr_val >= stack_base && fault_addr_val < stack_top {
        let user_pt = super::trap::get_current_user_pt();
        if crate::mm::vmm::translate_user(user_pt, page_addr).is_none() {
            if let Some(frame) = crate::mm::pmm::alloc_frame() {
                unsafe {
                    core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                }
                crate::mm::vmm::map(user_pt, page_addr, frame, crate::mm::vmm::PTEFlags::URW);
                super::trap::flush_tlb_addr(page_addr);
                return;
            }
        }
        return;
    }

    // Try lazy allocation for mmap region
    if from_user && fault_addr_val >= mmap_base && fault_addr_val < mmap_limit {
        let user_pt = super::trap::get_current_user_pt();
        if crate::mm::vmm::translate_user(user_pt, page_addr).is_none() {
            if let Some(frame) = crate::mm::pmm::alloc_frame() {
                unsafe {
                    core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                }
                crate::mm::vmm::map(user_pt, page_addr, frame, crate::mm::vmm::PTEFlags::URW);
                super::trap::flush_tlb_addr(page_addr);
                return;
            }
        }
        return;
    }

    // Fatal page fault
    crate::console_println!(
        "[EXCEPTION] Page Fault at {:#x}, accessing {:#x}, CR3={:#x}",
        frame.instruction_pointer.as_u64(),
        fault_addr_val,
        cr3,
    );
    if from_user {
        crate::console_println!("[trap] Killing user process");
        crate::syscall::dispatch(1, [1, 0, 0, 0, 0, 0]); // sys_exit(1)
    } else {
        loop {}
    }
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] Invalid Opcode at {:#x}",
        frame.instruction_pointer.as_u64()
    );
}

// ─── Hardware Interrupts ─────────────────────────────────────

/// Timer ISR stub — naked function that saves complete TrapContext.
#[unsafe(naked)]
unsafe extern "C" fn timer_handler() {
    unsafe {
        core::arch::naked_asm!(
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

            "mov rdi, rsp",
            "call {}",

            "call {}",    // lapic::local_eoi()

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

            sym timer_trap_handler,
            sym super::lapic::local_eoi,
        );
    }
}

/// Rust handler for timer interrupt trap.
unsafe extern "C" fn timer_trap_handler(ctx: &mut super::trap::TrapContext) {
    let _ = ctx;

    crate::driver::tty::poll_uart();
    crate::arch::platform::tick_uptime();

    #[cfg(target_arch = "riscv64")]
    crate::net::iface::NetStack::poll();

    super::lapic::set_next_timer();

    crate::sched::schedule();

    // After schedule(), update syscall KSP for the new task
    let target_root = crate::process::current_page_table_root();
    if target_root != 0 {
        let target_paddr = (target_root << 12) as u64;
        super::trap::activate_page_table(target_paddr as usize);
    }

    // Update the fast syscall entry kernel stack pointer
    let ksp = crate::sched::current_kernel_sp();
    if ksp != 0 {
        set_syscall_ksp(ksp as u64);
    }
}

extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    let scancode: u8 = unsafe { x86_64::instructions::port::Port::new(0x60).read() };
    crate::driver::keyboard::handle_scancode(scancode);
    super::lapic::local_eoi();
}

extern "x86-interrupt" fn com1_handler(_frame: InterruptStackFrame) {
    crate::driver::tty::poll_uart();
    super::lapic::local_eoi();
}

extern "x86-interrupt" fn spurious_handler(_frame: InterruptStackFrame) {
    // Spurious — don't EOI
}

// ─── Syscall ISR Stub (int 0x80, legacy path) ─────────────────

/// Rust syscall handler called from assembly.
unsafe extern "C" fn syscall_handler_impl(state_ptr: *const u64) -> u64 {
    unsafe {
        let s = state_ptr;
        let rdi = *s.add(0) as usize;
        let rsi = *s.add(1) as usize;
        let rdx = *s.add(2) as usize;
        let r10 = *s.add(6) as usize;
        let r8 = *s.add(4) as usize;
        let r9 = *s.add(5) as usize;
        let rax = *s.add(8) as usize;

        let syscall_id = rax;
        let args = [rdi, rsi, rdx, r10, r8, r9];

        let result = crate::syscall::dispatch(syscall_id, args);
        result as u64
    }
}

/// Naked ISR stub for int 0x80. Saves all GP regs, calls handler, restores, iretqs.
#[unsafe(naked)]
unsafe extern "C" fn syscall_isr_stub() {
    unsafe {
        core::arch::naked_asm!(
            "cli",
            "push rax",
            "push r11",
            "push r10",
            "push r9",
            "push r8",
            "push rcx",
            "push rdx",
            "push rsi",
            "push rdi",

            "mov rdi, rsp",
            "call {}",

            "mov [rsp + 64], rax",

            "pop rdi",
            "pop rsi",
            "pop rdx",
            "pop rcx",
            "pop r8",
            "pop r9",
            "pop r10",
            "pop r11",
            "pop rax",

            "sti",
            "iretq",
            sym syscall_handler_impl,
        );
    }
}

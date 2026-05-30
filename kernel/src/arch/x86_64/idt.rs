//! IDT setup using the `x86_64` crate.
//!
//! CPU exceptions and hardware interrupts use `extern "x86-interrupt"` handlers.
//! Syscall (int 0x80) uses a custom naked ISR stub for full register control.

use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

static IDT: Once<InterruptDescriptorTable> = Once::new();

pub const IRQ_BASE: u8 = 32;
pub const SYSCALL_VECTOR: u8 = 0x80;
pub const TIMER_VECTOR: u8 = IRQ_BASE + 0;
pub const KEYBOARD_VECTOR: u8 = IRQ_BASE + 1;
pub const COM1_VECTOR: u8 = IRQ_BASE + 4;
pub const SPURIOUS_VECTOR: u8 = IRQ_BASE + 7;

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
            // IDT entry 16 bytes:
            //   lo (8 bytes): offset_lo[15:0] | selector[31:16] | ist_attr[47:32] | offset_mid[63:48]
            //   hi (8 bytes): offset_hi[31:0] | reserved[63:32]
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
}

/// Load the already-built IDT. Called by APs to load the shared IDT.
pub fn load() {
    if let Some(idt) = IDT.get() {
        idt.load();
    }
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

    // Lazy page allocation: check if fault is in user heap area
    let heap_base = crate::process::USER_HEAP_BASE;
    let heap_limit = crate::process::USER_HEAP_LIMIT;
    let from_user = frame.code_segment.0 as u64 & 0x3 != 0;

    if from_user && fault_addr_val >= heap_base && fault_addr_val < heap_limit {
        let page_size = crate::mm::pmm::page_size();
        let page_addr = fault_addr_val & !(page_size - 1);

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
                return; // Retry the faulting instruction
            }
        }
        // Page already mapped or OOM — still retry
        return;
    }

    // Not in heap area — fatal page fault
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
///
/// This replaces `extern "x86-interrupt"` with a custom naked ISR because
/// we need to save ALL 15 GP registers (not just callee-saved) to safely
/// call `schedule()` → `__switch()` from the timer interrupt.
///
/// The CPU automatically pushes the iretq frame (SS, RSP, RFLAGS, CS, RIP)
/// before entering this handler. We then push all GP regs to build a
/// complete TrapContext on the kernel stack.
///
/// After trap_handler returns (which may call schedule → __switch),
/// we restore GP regs and iretq back to the interrupted context.
#[unsafe(naked)]
unsafe extern "C" fn timer_handler() {
    unsafe {
        core::arch::naked_asm!(
            // ── Save all 15 GP registers ──
            // Order must match TrapContext field order exactly.
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

            // RSP now points to TrapContext base (rax field).
            // The iretq frame (RIP, CS, RFLAGS, RSP, SS) is below GP regs
            // because CPU pushed them before us.

            // ── Call trap_handler with &mut TrapContext ──
            "mov rdi, rsp",
            "call {}",

            // ── After trap_handler (may have done schedule/__switch) ──
            // EOI must happen AFTER schedule returns to this context.
            "call {}",    // lapic::local_eoi()

            // ── Restore all 15 GP registers ──
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

            // ── Return from interrupt ──
            "iretq",

            sym timer_trap_handler,
            sym super::lapic::local_eoi,
        );
    }
}

/// Rust handler for timer interrupt trap.
/// Called from naked timer ISR with a pointer to TrapContext.
unsafe extern "C" fn timer_trap_handler(ctx: &mut super::trap::TrapContext) {
    let _ = ctx; // TrapContext is on stack, will be restored by assembly

    // Poll UART for input
    crate::driver::tty::poll_uart();

    // Reset timer (periodic mode — no-op, but call for consistency)
    super::lapic::set_next_timer();

    // ⚠️ EOI is done AFTER schedule returns (in assembly), not here.
    // This prevents another timer interrupt from firing while we're
    // still in the middle of context switching.

    // Preemptive scheduling: Round-Robin to next ready task
    crate::sched::schedule();

    // After schedule() returns (possibly on a different task's stack),
    // restore CR3 to the current process's user page table.
    // schedule() may have switched to a different process whose CR3 differs.
    let target_root = crate::process::current_page_table_root();
    if target_root != 0 {
        let target_paddr = (target_root << 12) as u64;
        super::trap::activate_page_table(target_paddr as usize);
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

// ─── Syscall ISR Stub (custom naked) ────────────────────────
//
// We use a custom naked ISR stub because we need to know the exact register
// save order to extract syscall args. The IDT entry points here.
//
// Stack on entry (CPU pushes): SS, RSP, RFLAGS, CS, RIP  (iretq frame)
// We push all GP regs in a known order, call the handler, restore, iretq.

/// Rust syscall handler called from assembly.
unsafe extern "C" fn syscall_handler_impl(state_ptr: *const u64) -> u64 {
    unsafe {
        // Register save order from assembly (push order, reversed for array):
        // push rdi [0], rsi [1], rdx [2], rcx [3], r8 [4], r9 [5],
        //       r10 [6], r11 [7], rax [8]
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
/// Referenced by the manually-constructed IDT entry.
#[unsafe(naked)]
unsafe extern "C" fn syscall_isr_stub() {
    unsafe {
        core::arch::naked_asm!(
            // Disable interrupts — we don't want timer interrupt during syscall
            "cli",

            // Save all GP registers in known order
            "push rax",          // [8]
            "push r11",          // [7]
            "push r10",          // [6]
            "push r9",           // [5]
            "push r8",           // [4]
            "push rcx",          // [3]
            "push rdx",          // [2]
            "push rsi",          // [1]
            "push rdi",          // [0]

            // Call Rust handler with pointer to saved state
            "mov rdi, rsp",
            "call {}",

            // Result is in rax. Write it to the saved rax slot (rsp + 8*8 = rsp + 64).
            "mov [rsp + 64], rax",

            // Restore all GP registers
            "pop rdi",
            "pop rsi",
            "pop rdx",
            "pop rcx",
            "pop r8",
            "pop r9",
            "pop r10",
            "pop r11",
            "pop rax",

            // Re-enable interrupts and return
            "sti",
            "iretq",
            sym syscall_handler_impl,
        );
    }
}

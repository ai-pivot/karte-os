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
        idt[TIMER_VECTOR].set_handler_fn(timer_handler);
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
    // On x86_64, GP faults during early bringup can be caused by many things.
    // Don't use console_println here — it can cause nested faults if the
    // UART lock is held. Just skip the instruction and continue.
    #[cfg(target_arch = "x86_64")]
    {
        // Skip the faulting instruction (int 0x80 is 2 bytes, others vary)
        // For now, just loop — this shouldn't happen in normal operation
        let _ = frame;
        let _ = err;
        loop {}
    }
    #[cfg(target_arch = "riscv64")]
    {
        crate::console_println!(
            "[EXCEPTION] GP Fault at {:#x}, err={:#x}",
            frame.instruction_pointer.as_u64(),
            err
        );
    }
}

extern "x86-interrupt" fn page_fault_handler(frame: InterruptStackFrame, _err: PageFaultErrorCode) {
    let fault_addr = x86_64::registers::control::Cr2::read();
    let cr3 = unsafe {
        x86_64::registers::control::Cr3::read()
            .0
            .start_address()
            .as_u64()
    };
    crate::console_println!(
        "[EXCEPTION] Page Fault at {:#x}, accessing {:#x}, CR3={:#x}",
        frame.instruction_pointer.as_u64(),
        fault_addr.map(|a| a.as_u64()).unwrap_or(0),
        cr3,
    );
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] Invalid Opcode at {:#x}",
        frame.instruction_pointer.as_u64()
    );
}

// ─── Hardware Interrupts ─────────────────────────────────────

extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    super::lapic::local_eoi();
    crate::driver::tty::poll_uart();
    super::lapic::set_next_timer();
    // Don't call schedule() for now — context switching from interrupt
    // context requires full TrapContext save/restore which isn't set up yet.
    // TODO: Implement proper context switching from interrupt context.
}

extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    let _scancode: u8 = unsafe { x86_64::instructions::port::Port::new(0x60).read() };
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

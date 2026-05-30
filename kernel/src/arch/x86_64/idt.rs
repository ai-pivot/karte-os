//! IDT (Interrupt Descriptor Table) setup using the `x86_64` crate.
//!
//! The `x86_64` crate provides `extern "x86-interrupt"` ABI support:
//! the compiler automatically generates prologue/epilogue that saves and
//! restores all registers, so most handlers can be pure Rust.
//!
//! For syscalls and context switching we need custom ISR stubs that
//! save additional register state (TrapContext).

use spin::Once;
use x86_64::structures::idt::{
    InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode,
};

static IDT: Once<InterruptDescriptorTable> = Once::new();

/// Hardware interrupt vector base (IRQ0 = vector 32, remapped by PIC).
pub const IRQ_BASE: u8 = 32;

/// System call interrupt vector (int 0x80, Linux-compatible).
pub const SYSCALL_VECTOR: u8 = 0x80;

/// Timer interrupt vector (IRQ0 remapped to 32).
pub const TIMER_VECTOR: u8 = IRQ_BASE + 0;

/// Keyboard interrupt vector (IRQ1 remapped to 33).
pub const KEYBOARD_VECTOR: u8 = IRQ_BASE + 1;

/// COM1 UART interrupt vector (IRQ4 remapped to 36).
pub const COM1_VECTOR: u8 = IRQ_BASE + 4;

/// Spurious interrupt vector (IRQ7, spurious in PIC).
pub const SPURIOUS_VECTOR: u8 = IRQ_BASE + 7;

/// Initialize the IDT and load it.
pub fn init() {
    IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        // CPU exception handlers
        idt.divide_error.set_handler_fn(divide_error_handler);
        idt.debug.set_handler_fn(debug_handler);
        idt.non_maskable_interrupt.set_handler_fn(nmi_handler);
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.overflow.set_handler_fn(overflow_handler);
        idt.bound_range_exceeded.set_handler_fn(bound_range_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.device_not_available.set_handler_fn(device_not_available_handler);
        // Double fault uses IST[0] for a dedicated stack
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(super::gdt::DOUBLE_FAULT_IST_INDEX);
        idt.invalid_tss.set_handler_fn(invalid_tss_handler);
        idt.segment_not_present.set_handler_fn(segment_not_present_handler);
        idt.stack_segment_fault
            .set_handler_fn(stack_segment_fault_handler);
        idt.general_protection_fault
            .set_handler_fn(gp_fault_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.x87_floating_point
            .set_handler_fn(x87_floating_point_handler);
        idt.alignment_check.set_handler_fn(alignment_check_handler);
        idt.machine_check.set_handler_fn(machine_check_handler);
        idt.simd_floating_point
            .set_handler_fn(simd_floating_point_handler);
        idt.virtualization.set_handler_fn(virtualization_handler);
        idt.security_exception
            .set_handler_fn(security_exception_handler);

        // Hardware interrupt handlers
        idt[TIMER_VECTOR as usize].set_handler_fn(timer_handler);
        idt[KEYBOARD_VECTOR as usize].set_handler_fn(keyboard_handler);
        idt[COM1_VECTOR as usize].set_handler_fn(com1_handler);
        idt[SPURIOUS_VECTOR as usize].set_handler_fn(spurious_handler);

        // Syscall via int 0x80
        // Note: `extern "x86-interrupt"` doesn't give us access to all GP regs.
        // For full syscall dispatch we need a custom ISR stub (see trap.rs).
        // This is a placeholder; the real syscall path goes through a naked
        // function wrapper defined in trap.rs.
        idt[SYSCALL_VECTOR as usize].set_handler_fn(syscall_int_handler);

        idt
    });

    IDT.get().unwrap().load();
}

// ─── CPU Exception Handlers ─────────────────────────────────

unsafe extern "x86-interrupt" fn divide_error_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] Divide Error at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn debug_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] Debug at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn nmi_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] NMI at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] Breakpoint at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn overflow_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] Overflow at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn bound_range_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] Bound Range Exceeded at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] Invalid Opcode at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn device_not_available_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] Device Not Available at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn double_fault_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    panic!(
        "[EXCEPTION] Double Fault at {:#x}, error code={:#x}",
        frame.instruction_pointer.as_u64(),
        error_code,
    );
}

unsafe extern "x86-interrupt" fn invalid_tss_handler(frame: InterruptStackFrame, error_code: u64) {
    crate::console_println!(
        "[EXCEPTION] Invalid TSS at {:#x}, error code={:#x}",
        frame.instruction_pointer.as_u64(),
        error_code,
    );
}

unsafe extern "x86-interrupt" fn segment_not_present_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    crate::console_println!(
        "[EXCEPTION] Segment Not Present at {:#x}, error code={:#x}",
        frame.instruction_pointer.as_u64(),
        error_code,
    );
}

unsafe extern "x86-interrupt" fn stack_segment_fault_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    crate::console_println!(
        "[EXCEPTION] Stack Segment Fault at {:#x}, error code={:#x}",
        frame.instruction_pointer.as_u64(),
        error_code,
    );
}

unsafe extern "x86-interrupt" fn gp_fault_handler(frame: InterruptStackFrame, error_code: u64) {
    crate::console_println!(
        "[EXCEPTION] General Protection Fault at {:#x}, error code={:#x}",
        frame.instruction_pointer.as_u64(),
        error_code,
    );
}

unsafe extern "x86-interrupt" fn page_fault_handler(
    frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let fault_addr = x86_64::registers::control::Cr2::read();
    crate::console_println!(
        "[EXCEPTION] Page Fault at {:#x}, accessing {:#x}, error={:?}",
        frame.instruction_pointer.as_u64(),
        fault_addr.as_u64(),
        error_code,
    );
    // TODO: forward to the kernel's page fault handler for lazy allocation
}

unsafe extern "x86-interrupt" fn x87_floating_point_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] x87 Floating Point at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn alignment_check_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    crate::console_println!(
        "[EXCEPTION] Alignment Check at {:#x}, error code={:#x}",
        frame.instruction_pointer.as_u64(),
        error_code,
    );
}

unsafe extern "x86-interrupt" fn machine_check_handler(frame: InterruptStackFrame) -> ! {
    panic!(
        "[EXCEPTION] Machine Check at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn simd_floating_point_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] SIMD Floating Point at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn virtualization_handler(frame: InterruptStackFrame) {
    crate::console_println!(
        "[EXCEPTION] Virtualization at {:#x}",
        frame.instruction_pointer.as_u64(),
    );
}

unsafe extern "x86-interrupt" fn security_exception_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    crate::console_println!(
        "[EXCEPTION] Security Exception at {:#x}, error code={:#x}",
        frame.instruction_pointer.as_u64(),
        error_code,
    );
}

// ─── Hardware Interrupt Handlers ─────────────────────────────

unsafe extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    // LAPIC timer tick — drive the scheduler
    crate::arch::lapic::local_eoi();
    // Poll UART (like RISC-V timer handler)
    crate::driver::tty::poll_uart();
    // Set next timer tick
    super::lapic::set_next_timer();
    // Invoke round-robin scheduler
    crate::sched::schedule();
}

unsafe extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    // Read scan code from keyboard controller
    let _scancode: u8 = unsafe { x86_64::instructions::port::Port::new(0x60).read() };
    // TODO: keyboard input handling
    super::lapic::local_eoi();
}

unsafe extern "x86-interrupt" fn com1_handler(_frame: InterruptStackFrame) {
    // COM1 UART interrupt — drain RX FIFO into TTY ring buffer
    crate::driver::tty::poll_uart();
    super::lapic::local_eoi();
}

unsafe extern "x86-interrupt" fn spurious_handler(_frame: InterruptStackFrame) {
    // Spurious interrupt — ignore (don't send EOI for PIC spurious)
}

// ─── Syscall Handler (int 0x80) ──────────────────────────────

unsafe extern "x86-interrupt" fn syscall_int_handler(frame: InterruptStackFrame) {
    // NOTE: `extern "x86-interrupt"` only gives us the InterruptStackFrame,
    // not the full register state. For a real syscall dispatch we need a
    // custom ISR stub that saves all GP registers into a TrapContext.
    // This is a placeholder. The actual syscall path uses a naked wrapper
    // defined in trap.rs that builds a full TrapContext before calling
    // the Rust trap_handler.
    //
    // For now, read syscall number from rax via the frame (not available
    // in InterruptStackFrame — need custom approach).
    crate::console_println!(
        "[SYSCALL] int 0x80 at {:#x} (custom ISR stub needed for register access)",
        frame.instruction_pointer.as_u64(),
    );
}

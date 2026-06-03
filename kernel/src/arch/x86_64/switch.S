//! Context switch assembly for x86_64.
//!
//! This is the ONLY assembly file besides boot.S. It saves/restores
//! callee-saved registers (rbx, rbp, r12-r15) and FPU/SSE state
//! for preemptive context switching between kernel tasks.
//!
//! Equivalent to the RISC-V switch.S which saves ra + s0-s11 (13 regs).
//! x86_64 calling convention callee-saved: rbx, rbp, r12, r13, r14, r15 (6 regs + ra = 7)
//!
//! Layout on stack (56 + 8 + 512 = 576 bytes):
//!   offset 0:    rip (return address, pushed by `call`)
//!   offset 8:    rbp
//!   offset 16:   rbx
//!   offset 24:   r12
//!   offset 32:   r13
//!   offset 40:   r14
//!   offset 48:   r15
//!   offset 56:   dummy qword (for 16-byte alignment)
//!   offset 64:   fxsave area (512 bytes, must be 16-byte aligned)

.section .text
.global __switch
.type __switch, @function
.align 8

# __switch(current_sp: &mut usize, next_sp: *const usize)
#   rdi = pointer to current task's saved stack pointer
#   rsi = pointer to next task's saved stack pointer
__switch:
    # Save callee-saved registers onto current stack
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15

    # Save FPU/SSE state (512 bytes, requires 16-byte alignment)
    # After 6 pushes (48 bytes) + return address (8 bytes) = 56 bytes,
    # RSP is 8 mod 16. Push a dummy qword to make it 0 mod 16 (aligned).
    xor rax, rax
    push rax            # dummy for 16-byte alignment
    sub rsp, 512
    fxsave64 [rsp]

    # Save current stack pointer into *current_sp (rdi)
    mov [rdi], rsp

    # Load next stack pointer from *next_sp (rsi)
    mov rsp, [rsi]

    # Restore FPU/SSE state
    fxrstor64 [rsp]
    add rsp, 512

    # Remove alignment dummy
    add rsp, 8

    # Restore callee-saved registers from next stack
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp

    # Return — jumps to the saved return address of the next task
    ret

.size __switch, . - __switch

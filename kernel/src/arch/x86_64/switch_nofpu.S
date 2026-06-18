//! Lightweight context switch (no FPU save/restore) for x86_64.
//!
//! Same stack frame layout as __switch — the fxsave area (512 bytes) is
//! allocated but NOT read or written. This makes it fully compatible with
//! the regular __switch: either function can switch to a stack created by
//! the other.
//!
//! After switching, CR0.TS is set so any FPU instruction triggers #NM.
//! The #NM handler performs the actual fxsave/fxrstor on demand.
//!
//! Stack frame layout (same as __switch):
//!   switch_sp ──► ┌──────────────────┐
//!                 │ fxsave (512B)    │ ← skipped (not read/written)
//!                 ├──────────────────┤ +512
//!                 │ orig_rsp (8B)    │
//!                 ├──────────────────┤ +520
//!                 │ r15..rbp (48B)   │
//!                 ├──────────────────┤ +568
//!                 │ return addr (8B) │
//!                 └──────────────────┘ +576

.section .text.__switch_no_fpu, "ax", @progbits
.global __switch_no_fpu
.protected __switch_no_fpu
.type __switch_no_fpu, @function
.align 8

# __switch_no_fpu(current_sp: &mut usize, next_sp: *const usize)
#   rdi = pointer to current task's saved stack pointer
#   rsi = pointer to next task's saved stack pointer
__switch_no_fpu:
    cli

    # ─── SAVE path (current task) ────────────────────────────
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15

    # Align RSP and reserve fxsave area (same as __switch, but DON'T fxsave)
    mov rax, rsp
    and rsp, ~0xF
    sub rsp, 512
    # Skip fxsave64 — FPU state not saved here

    # Save original RSP (same offset as __switch)
    mov [rsp + 512], rax

    # Store current frame pointer
    mov [rdi], rsp

    # ─── RESTORE path (next task) ────────────────────────────
    mov rsp, [rsi]

    # Skip fxrstor64 — DON'T restore FPU state

    # Recover original RSP (same offset as __switch)
    mov rsp, [rsp + 512]

    # Set CR0.TS (Task Switched bit) so FPU instructions trigger #NM
    mov rax, cr0
    or rax, 8        # CR0_TS = bit 3
    mov cr0, rax

    # Restore callee-saved registers
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp

    ret

.size __switch_no_fpu, . - __switch_no_fpu

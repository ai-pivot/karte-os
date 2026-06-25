//! Context switch assembly for x86_64.
//!
//! Saves/restores callee-saved registers (rbx, rbp, r12-r15) and FPU/SSE
//! state (fxsave64, 512 bytes) for preemptive context switching.
//!
//! Stack frame layout (from switch_sp upward):
//!
//!   switch_sp ──► ┌──────────────────┐
//!                 │ fxsave (512B)    │ ← 16-byte aligned
//!                 │                  │
//!                 ├──────────────────┤ +512
//!                 │ orig_rsp (8B)    │ saved original RSP for restore
//!                 ├──────────────────┤ +520
//!                 │ r15 (8B)         │
//!                 │ r14 (8B)         │
//!                 │ r13 (8B)         │
//!                 │ r12 (8B)         │
//!                 │ rbx (8B)         │
//!                 │ rbp (8B)         │
//!                 ├──────────────────┤ +568
//!                 │ return addr (8B) │
//!                 └──────────────────┘ +576 = total frame
//!
//! Total extra below callee-saved: 512 (fxsave) + 8 (orig_rsp) = 520 bytes

.section .text.__switch, "ax", @progbits
.global __switch
.protected __switch
.type __switch, @function
.align 8

# __switch(current_sp: &mut usize, next_sp: *const usize)
#   rdi = pointer to current task's saved stack pointer
#   rsi = pointer to next task's saved stack pointer
__switch:
    # ─── CRITICAL: Disable interrupts during context switch ───
    cli

    # DIAG 12: __switch entered
    push rdi
    push rsi
    sub rsp, 8
    mov edi, 12
    mov esi, 0x00FF8000
    call fb_debug_square
    add rsp, 8
    pop rsi
    pop rdi

    # ─── SAVE path (current task) ────────────────────────────
    # Save callee-saved registers
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    # Stack now has: ret_addr, rbp, rbx, r12, r13, r14, r15 = 7 pushes (56 bytes)
    # RSP = original_RSP - 56. RSP mod 16 could be 0 or 8.

    # Align RSP to 16 bytes for fxsave64, saving original RSP for later
    mov rax, rsp            # rax = current RSP (before alignment)
    and rsp, ~0xF           # align to 16 bytes (may move 0 or 8 bytes down)
    sub rsp, 512            # make room for fxsave area (16-byte aligned)

    # DIAG 13: before fxsave64
    push rdi
    push rsi
    sub rsp, 8
    mov edi, 13
    mov esi, 0x008000FF
    call fb_debug_square
    add rsp, 8
    pop rsi
    pop rdi

    fxsave64 [rsp]

    # DIAG 14: after fxsave64
    push rdi
    push rsi
    sub rsp, 8
    mov edi, 14
    mov esi, 0x000080FF
    call fb_debug_square
    add rsp, 8
    pop rsi
    pop rdi

    # Save original RSP above fxsave area so restore path can find it
    mov [rsp + 512], rax

    # Store current frame pointer for later restore
    mov [rdi], rsp

    # ─── RESTORE path (next task) ────────────────────────────
    # Load next task's frame pointer
    mov rsp, [rsi]

    # DIAG 15: before fxrstor64
    push rdi
    push rsi
    sub rsp, 8
    mov edi, 15
    mov esi, 0x0000FF80
    call fb_debug_square
    add rsp, 8
    pop rsi
    pop rdi

    # Restore FPU/SSE state (rsp is 16-byte aligned, set during save path)
    fxrstor64 [rsp]

    # DIAG 16: after fxrstor64
    push rdi
    push rsi
    sub rsp, 8
    mov edi, 16
    mov esi, 0x0080FF00
    call fb_debug_square
    add rsp, 8
    pop rsi
    pop rdi

    # Recover original RSP (the value before alignment was applied)
    mov rsp, [rsp + 512]

    # High-half kernel: all code addresses are in the upper canonical half
    # (0xFFFF_FFFF_8000_0000+). No range check needed — the return address
    # is always valid because it was set by build_initial_stack or the call
    # instruction. The old [1MB, 4MB) range check was for the identity-mapped
    # kernel and is obsolete.

    # Restore callee-saved registers
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp

    # Return — jumps to the saved return address of the next task
    ret

.size __switch, . - __switch

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
    fxsave64 [rsp]          # ✓ RSP is 16-byte aligned

    # Save original RSP above fxsave area so restore path can find it
    mov [rsp + 512], rax

    # Store current frame pointer for later restore
    mov [rdi], rsp

    # ─── RESTORE path (next task) ────────────────────────────
    # Load next task's frame pointer
    mov rsp, [rsi]

    # Restore FPU/SSE state (rsp is 16-byte aligned, set during save path)
    fxrstor64 [rsp]

    # Recover original RSP (the value before alignment was applied)
    mov rsp, [rsp + 512]

    # Debug: validate return address before restoring
    # Kernel code region (may grow, keep range generous)
    mov rax, [rsp + 48]    # return address is at orig_rsp + 48
    cmp rax, 0x100000
    jb .Lswitch_bad_ret
    cmp rax, 0x400000
    jae .Lswitch_bad_ret
    jmp .Lswitch_restore_ok
.Lswitch_bad_ret:
    # Invalid return address! Print BADSW to VGA and halt
    mov rsi, [rsp + 48]    # bad return address
    mov rdi, rsp            # orig_rsp
    mov byte ptr [0xB8000], 'B'
    mov byte ptr [0xB8002], 'A'
    mov byte ptr [0xB8004], 'D'
    mov byte ptr [0xB8006], 'S'
    mov byte ptr [0xB8008], 'W'
    cli
    hlt
.Lswitch_restore_ok:

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

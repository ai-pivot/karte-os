// trap_entry.S — Trap entry/exit assembly for U-mode ↔ S-mode transitions
//
// Dual-path trap handling:
//   - Uses sscratch CSR to distinguish trap source
//   - S-mode traps: sscratch=0, normal kernel trap handling
//   - U-mode traps: sscratch=kernel_sp, swap stacks and save user context
//
// TrapContext layout (280 bytes):
//   x[0..32]  @ offset 0..256   — 32 general-purpose registers
//   sstatus   @ offset 256
//   sepc      @ offset 264
//   sscratch  @ offset 272      — user sp if from U-mode, else 0

    .section .text
    .globl trap_entry

trap_entry:
    # ── Step 1: Swap sp with sscratch ──
    # After csrrw: sp = old_sscratch, sscratch = old_sp
    #   S-mode: sp = 0,         sscratch = S-mode sp
    #   U-mode: sp = kernel_sp,  sscratch = user_sp
    csrrw   sp, sscratch, sp

    # ── Step 2: Check trap source via old sscratch value (now in sp) ──
    bnez    sp, .Ltrap_from_user

    # ═══════════════════════════════════════════════════════════════
    # S-mode trap path
    # sp = 0 (old sscratch), sscratch = old S-mode sp
    # ═══════════════════════════════════════════════════════════════
    csrr    sp, sscratch           # restore S-mode sp
    csrw    sscratch, zero         # clear sscratch for next trap

    addi    sp, sp, -280

    # Save general-purpose registers
    sd      x1,  8(sp)
    sd      x3,  24(sp)
    sd      x4,  32(sp)
    sd      x5,  40(sp)
    sd      x6,  48(sp)
    sd      x7,  56(sp)
    sd      x8,  64(sp)
    sd      x9,  72(sp)
    sd      x10, 80(sp)
    sd      x11, 88(sp)
    sd      x12, 96(sp)
    sd      x13, 104(sp)
    sd      x14, 112(sp)
    sd      x15, 120(sp)
    sd      x16, 128(sp)
    sd      x17, 136(sp)
    sd      x18, 144(sp)
    sd      x19, 152(sp)
    sd      x20, 160(sp)
    sd      x21, 168(sp)
    sd      x22, 176(sp)
    sd      x23, 184(sp)
    sd      x24, 192(sp)
    sd      x25, 200(sp)
    sd      x26, 208(sp)
    sd      x27, 216(sp)
    sd      x28, 224(sp)
    sd      x29, 232(sp)
    sd      x30, 240(sp)
    sd      x31, 248(sp)

    # Save CSRs
    csrr    t1, sstatus
    sd      t1, 256(sp)
    csrr    t1, sepc
    sd      t1, 264(sp)
    sd      zero, 272(sp)          # sscratch field = 0 (S-mode)

    # Call trap_handler(&mut TrapContext)
    mv      a0, sp
    call    trap_handler

    # Restore CSRs
    ld      t1, 264(sp)
    csrw    sepc, t1
    ld      t1, 256(sp)
    csrw    sstatus, t1

    # Restore general-purpose registers
    ld      x1,  8(sp)
    ld      x3,  24(sp)
    ld      x4,  32(sp)
    ld      x5,  40(sp)
    ld      x6,  48(sp)
    ld      x7,  56(sp)
    ld      x8,  64(sp)
    ld      x9,  72(sp)
    ld      x10, 80(sp)
    ld      x11, 88(sp)
    ld      x12, 96(sp)
    ld      x13, 104(sp)
    ld      x14, 112(sp)
    ld      x15, 120(sp)
    ld      x16, 128(sp)
    ld      x17, 136(sp)
    ld      x18, 144(sp)
    ld      x19, 152(sp)
    ld      x20, 160(sp)
    ld      x21, 168(sp)
    ld      x22, 176(sp)
    ld      x23, 184(sp)
    ld      x24, 192(sp)
    ld      x25, 200(sp)
    ld      x26, 208(sp)
    ld      x27, 216(sp)
    ld      x28, 224(sp)
    ld      x29, 232(sp)
    ld      x30, 240(sp)
    ld      x31, 248(sp)

    addi    sp, sp, 280
    sret

    # ═══════════════════════════════════════════════════════════════
    # U-mode trap path
    # sp = kernel_sp (from sscratch), sscratch = user_sp (old sp)
    # ═══════════════════════════════════════════════════════════════
.Ltrap_from_user:
    # Save user_sp from sscratch into t0 before clearing
    csrr    t0, sscratch           # t0 = user_sp
    csrw    sscratch, zero         # clear sscratch

    addi    sp, sp, -280

    # Save general-purpose registers
    sd      x1,  8(sp)
    sd      t0,  16(sp)            # x[2] = user_sp (from sscratch)
    sd      x3,  24(sp)
    sd      x4,  32(sp)
    sd      x5,  40(sp)
    sd      x6,  48(sp)
    sd      x7,  56(sp)
    sd      x8,  64(sp)
    sd      x9,  72(sp)
    sd      x10, 80(sp)
    sd      x11, 88(sp)
    sd      x12, 96(sp)
    sd      x13, 104(sp)
    sd      x14, 112(sp)
    sd      x15, 120(sp)
    sd      x16, 128(sp)
    sd      x17, 136(sp)
    sd      x18, 144(sp)
    sd      x19, 152(sp)
    sd      x20, 160(sp)
    sd      x21, 168(sp)
    sd      x22, 176(sp)
    sd      x23, 184(sp)
    sd      x24, 192(sp)
    sd      x25, 200(sp)
    sd      x26, 208(sp)
    sd      x27, 216(sp)
    sd      x28, 224(sp)
    sd      x29, 232(sp)
    sd      x30, 240(sp)
    sd      x31, 248(sp)

    # Save CSRs
    csrr    t1, sstatus
    sd      t1, 256(sp)
    csrr    t1, sepc
    sd      t1, 264(sp)
    sd      t0,  272(sp)           # sscratch field = user_sp

    # Call trap_handler(&mut TrapContext)
    mv      a0, sp
    call    trap_handler

    # ── Return to U-mode ──

    # Restore CSRs
    ld      t1, 264(sp)
    csrw    sepc, t1
    ld      t1, 256(sp)
    csrw    sstatus, t1

    # Stash user_sp in sscratch before restoring GP registers
    # (restoring x5 would clobber t0, so we use the CSR as temporary storage)
    ld      t0, 16(sp)             # t0 = user_sp from x[2] slot
    csrw    sscratch, t0           # sscratch = user_sp (temporary)

    # Restore ALL general-purpose registers from kernel stack
    ld      x1,  8(sp)
    ld      x3,  24(sp)
    ld      x4,  32(sp)
    ld      x5,  40(sp)            # restores user's t0
    ld      x6,  48(sp)
    ld      x7,  56(sp)            # restores user's t2
    ld      x8,  64(sp)
    ld      x9,  72(sp)
    ld      x10, 80(sp)
    ld      x11, 88(sp)
    ld      x12, 96(sp)
    ld      x13, 104(sp)
    ld      x14, 112(sp)
    ld      x15, 120(sp)
    ld      x16, 128(sp)
    ld      x17, 136(sp)
    ld      x18, 144(sp)
    ld      x19, 152(sp)
    ld      x20, 160(sp)
    ld      x21, 168(sp)
    ld      x22, 176(sp)
    ld      x23, 184(sp)
    ld      x24, 192(sp)
    ld      x25, 200(sp)
    ld      x26, 208(sp)
    ld      x27, 216(sp)
    ld      x28, 224(sp)
    ld      x29, 232(sp)
    ld      x30, 240(sp)
    ld      x31, 248(sp)

    # All GP registers now have user-mode values.
    # sp still points to TrapContext on kernel stack.
    # sscratch holds user_sp.

    # Pop the TrapContext frame
    addi    sp, sp, 280            # sp = kernel_stack_top

    # Swap sp and sscratch via csrrw:
    #   tmp = sscratch (user_sp)   → returned to sp
    #   sscratch = sp (kernel_sp)  → set for next U-mode trap
    csrrw   sp, sscratch, sp

    # Result: sp = user_sp, sscratch = kernel_stack_top
    sret

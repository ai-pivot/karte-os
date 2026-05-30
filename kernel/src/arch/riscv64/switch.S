.section .text
.global __switch
# __switch(current_sp: &mut usize, next_sp: usize)
# a0 = &mut current task's stack pointer (save current sp here)
# a1 = next task's stack pointer (restore from this)
__switch:
    # Save callee-saved registers to current task's stack
    addi sp, sp, -104    # 13 registers * 8 bytes = 104

    sd ra, 0(sp)
    sd s0, 8(sp)
    sd s1, 16(sp)
    sd s2, 24(sp)
    sd s3, 32(sp)
    sd s4, 40(sp)
    sd s5, 48(sp)
    sd s6, 56(sp)
    sd s7, 64(sp)
    sd s8, 72(sp)
    sd s9, 80(sp)
    sd s10, 88(sp)
    sd s11, 96(sp)

    # Save current sp to *current_sp (a0)
    sd sp, 0(a0)

    # Restore sp from next_sp (a1)
    ld sp, 0(a1)

    # Restore callee-saved registers
    ld ra, 0(sp)
    ld s0, 8(sp)
    ld s1, 16(sp)
    ld s2, 24(sp)
    ld s3, 32(sp)
    ld s4, 40(sp)
    ld s5, 48(sp)
    ld s6, 56(sp)
    ld s7, 64(sp)
    ld s8, 72(sp)
    ld s9, 80(sp)
    ld s10, 88(sp)
    ld s11, 96(sp)

    addi sp, sp, 104
    ret

    .section .text
    .globl _start

_start:
    # ── Test 1: Query current brk ──
    li      a7, 4              # sys_brk
    li      a0, 0              # addr=0 → query current
    ecall

    # a0 = current brk (should be USER_HEAP_BASE = 0x400000)
    mv      s1, a0             # s1 = heap base

    # ── Test 2: Grow heap by one page ──
    li      a7, 4              # sys_brk
    li      t0, 4096
    add     a0, s1, t0         # new brk = current + 4096
    ecall

    # ── Test 3: Write small values to heap memory ──
    li      t0, 42
    sw      t0, 0(s1)          # heap[0] = 42
    li      t0, 99
    sw      t0, 4(s1)          # heap[4] = 99

    # ── Test 4: Read back and verify ──
    lw      t1, 0(s1)
    li      t2, 42
    bne     t1, t2, fail

    lw      t1, 4(s1)
    li      t2, 99
    bne     t1, t2, fail

    # ── Test 5: Print success ──
    la      a1, msg_ok
    li      a7, 2              # sys_write
    li      a0, 1              # fd=stdout
    li      a2, 15             # len
    ecall

    # ── Test 6: Grow heap by 8 more pages (32KB) ──
    li      a7, 4              # sys_brk
    li      t0, 32768
    add     a0, s1, t0
    ecall

    # Write near the end of the new allocation
    li      t0, 32764
    add     t0, s1, t0
    li      t1, 1234
    sw      t1, 0(t0)
    lw      t2, 0(t0)
    li      t3, 1234
    bne     t2, t3, fail

    # Print multi-page success
    la      a1, msg_multi
    li      a7, 2              # sys_write
    li      a0, 1
    li      a2, 21             # len
    ecall

    # ── Exit with code 0 ──
    li      a7, 1              # sys_exit
    li      a0, 0
    ecall

fail:
    la      a1, msg_fail
    li      a7, 2              # sys_write
    li      a0, 1
    li      a2, 12             # len
    ecall

    li      a7, 1              # sys_exit
    li      a0, 1
    ecall

    .section .rodata
msg_ok:
    .asciz "Heap basic OK!\n"
msg_multi:
    .asciz "Heap multi-page OK!\n"
msg_fail:
    .asciz "Heap FAILED!\n"

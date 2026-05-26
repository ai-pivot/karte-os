    .section .text
    .globl _start

# Syscall numbers
.equ SYS_EXIT,  1
.equ SYS_WRITE, 2
.equ SYS_READ,  3
.equ SYS_OPEN,  10
.equ SYS_CLOSE, 11

# File flags
.equ O_RDONLY,  0
.equ O_WRONLY,  1
.equ O_RDWR,    2
.equ O_CREAT,   0x100

_start:
    # ── Test 1: Create and open a file for writing ──
    la      a0, fname
    li      a1, 9              # len of "test.txt\0"
    li      a2, O_WRONLY | O_CREAT
    li      a7, SYS_OPEN       # sys_open
    ecall

    # Check fd >= 0
    bltz    a0, fail
    mv      s10, a0            # save fd

    # ── Test 2: Write data to file ──
    mv      a0, s10            # fd
    la      a1, wdata          # "Hello file!"
    li      a2, 11             # len
    li      a7, SYS_WRITE
    ecall

    # Check bytes written
    li      t0, 11
    bne     a0, t0, fail

    # ── Test 3: Close file ──
    mv      a0, s10
    li      a7, SYS_CLOSE
    ecall
    bltz    a0, fail

    # ── Test 4: Open file for reading ──
    la      a0, fname
    li      a1, 9
    li      a2, O_RDONLY
    li      a7, SYS_OPEN
    ecall
    bltz    a0, fail
    mv      s10, a0

    # ── Test 5: Read data back ──
    mv      a0, s10
    # Allocate a buffer on stack
    addi    sp, sp, -64
    mv      a1, sp
    li      a2, 64
    li      a7, SYS_READ
    ecall

    # Check bytes read (should be 11)
    li      t0, 11
    bne     a0, t0, fail

    # ── Test 6: Verify data matches ──
    li      t1, 0
    la      t2, wdata
verify_loop:
    lb      t3, 0(t2)          # expected byte
    lb      t4, 0(sp)          # actual byte (buffer at sp)
    bne     t3, t4, fail
    addi    t1, t1, 1
    addi    t2, t2, 1
    addi    sp, sp, 1
    li      t5, 11
    bne     t1, t5, verify_loop

    # Restore stack (we consumed 11 bytes of the 64 allocated)
    addi    sp, sp, -11        # undo the 11 increments
    addi    sp, sp, 64         # undo the initial -64

    # ── Test 7: Close file ──
    mv      a0, s10
    li      a7, SYS_CLOSE
    ecall

    # ── Print success ──
    la      a1, msg_ok
    li      a7, SYS_WRITE
    li      a0, 1
    li      a2, 18
    ecall

    # ── Exit success ──
    li      a7, SYS_EXIT
    li      a0, 0
    ecall

fail:
    la      a1, msg_fail
    li      a7, SYS_WRITE
    li      a0, 1
    li      a2, 17
    ecall

    li      a7, SYS_EXIT
    li      a0, 1
    ecall

    .section .rodata
fname:
    .asciz "test.txt\0"
wdata:
    .asciz "Hello file!"
msg_ok:
    .asciz "File test: all OK!\n"
msg_fail:
    .asciz "File test FAILED!\n"

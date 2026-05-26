# user/hello.S — Minimal user program for KarteOS
# Assembles to a raw ELF that runs in U-mode

.section .text
.globl _start

_start:
    # sys_write(fd=1, buf=msg, len=16)
    li      a7, 2           # SYS_WRITE
    li      a0, 1           # fd = stdout
    la      a1, msg         # buf
    li      a2, 16          # len
    ecall

    # sys_exit(code=0)
    li      a7, 1           # SYS_EXIT
    li      a0, 0           # code = 0
    ecall

    # Should not reach here
    j       .

.section .rodata
msg:
    .string "Hello from user!"

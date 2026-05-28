# user/shell.S — Minimal interactive shell for KarteOS
# Tests TTY input: prints prompt, reads a line (polling), echoes it back.
#
# sys_read(fd=0) is non-blocking: returns 0 if no line available.
# We poll in U-mode, which allows PLIC external interrupts to fire.

.section .text
.globl _start

_start:
    # Allocate a 256-byte input buffer on the stack
    addi    sp, sp, -256

loop:
    # Print prompt "$ "
    li      a7, 2           # SYS_WRITE
    li      a0, 1           # fd = stdout
    la      a1, prompt
    li      a2, 2           # len
    ecall

poll:
    # Try to read a line from stdin: read(0, buf, 254)
    li      a7, 3           # SYS_READ
    li      a0, 0           # fd = stdin
    mv      a1, sp          # buf = stack buffer
    li      a2, 254         # max len
    ecall

    # If read returned 0 (no data), poll again
    # This spinning is fine — we're in U-mode so PLIC interrupts
    # can fire and fill the TTY buffer.
    beqz    a0, poll

    # If read returned < 0 (error), restart loop
    bltz    a0, loop

    # Save read length
    mv      s1, a0

    # Print "you typed: "
    li      a7, 2
    li      a0, 1
    la      a1, echo_prefix
    li      a2, 12
    ecall

    # Echo what we read: write(1, buf, read_len)
    li      a7, 2           # SYS_WRITE
    li      a0, 1           # fd = stdout
    mv      a1, sp          # buf
    mv      a2, s1          # len
    ecall

    j       loop

.section .rodata
prompt:
    .string "$ "
echo_prefix:
    .string "you typed: "

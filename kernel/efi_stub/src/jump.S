# Naked assembly far jump function
# rdi = GDTR address, rsi = kernel entry point
.globl jump_to_kernel
jump_to_kernel:
    lgdt (%rdi)
    # Push return frame for lretq: [RIP, CS]
    pushq $0x08           # CS = 0x08 (64-bit code in our GDT)
    pushq %rsi            # RIP = kernel entry
    lretq

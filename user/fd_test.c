/* fd_test.c — minimal test */
void _start(void) {
    __asm__ volatile(
        "mov $1, %%rax\n"       /* SYS_write */
        "mov $2, %%rdi\n"       /* stderr */
        "lea 1f(%%rip), %%rsi\n"/* buf */
        "mov $3, %%rdx\n"       /* len */
        "int $0x80\n"
        "1: .ascii \"HI\" \n"   /* 2 bytes: H, I */
        ".byte 0x0a\n"          /* newline */
        /* openat test */
        "mov $257, %%rax\n"     /* SYS_openat */
        "mov $-100, %%rdi\n"    /* AT_FDCWD */
        "lea 2f(%%rip), %%rsi\n"/* path */
        "mov $0x241, %%rdx\n"   /* O_WRONLY|O_CREAT|O_TRUNC */
        "mov $0x1a4, %%r10\n"   /* mode 0644 */
        "int $0x80\n"           /* openat -> fd in rax */
        "mov %%rax, %%r12\n"    /* save fd1 */
        /* write result */
        "add $0x30, %%rax\n"    /* digit */
        "mov %%al, 3f\n"        /* store digit */
        "mov $1, %%rax\n"       /* SYS_write */
        "mov $2, %%rdi\n"       /* stderr */
        "lea 3f(%%rip), %%rsi\n"
        "mov $2, %%rdx\n"
        "int $0x80\n"
        /* 2nd openat — should return next fd */
        "mov $257, %%rax\n"
        "mov $-100, %%rdi\n"
        "lea 4f(%%rip), %%rsi\n"
        "mov $0x241, %%rdx\n"
        "mov $0x1a4, %%r10\n"
        "int $0x80\n"
        "mov %%rax, %%r13\n"    /* save fd2 */
        /* write result */
        "add $0x30, %%rax\n"
        "mov %%al, 5f\n"
        "mov $1, %%rax\n"
        "mov $2, %%rdi\n"
        "lea 5f(%%rip), %%rsi\n"
        "mov $2, %%rdx\n"
        "int $0x80\n"
        /* exit(0) */
        "mov $60, %%rax\n"
        "xor %%rdi, %%rdi\n"
        "int $0x80\n"
        "2: .asciz \"/t1.txt\"\n"
        "4: .asciz \"/t2.txt\"\n"
        "3: .byte 0x3f, 0x0a\n"   /* ?\n placeholder for fd1 digit */
        "5: .byte 0x3f, 0x0a\n"   /* ?\n placeholder for fd2 digit */
        ::: "rax", "rdi", "rsi", "rdx", "r10", "r12", "r13", "rcx", "memory"
    );
    __builtin_unreachable();
}

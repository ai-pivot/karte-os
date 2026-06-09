// Minimal test: create file then stat it
// Compile: x86_64-linux-gnu-gcc -static -nostdlib -o test_stat.elf test_stat.c
//
// Linux x86_64 syscall ABI via `syscall` instruction:
//   rax=nr, rdi=a1, rsi=a2, rdx=a3, r10=a4, r8=a5, r9=a6
//   return in rax

#define SYS_write   1
#define SYS_stat    4
#define SYS_exit   60
#define SYS_openat 257

static long syscall1(long n, long a1) {
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n), "D"(a1) : "rcx", "r11", "memory");
    return ret;
}

static long syscall2(long n, long a1, long a2) {
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n), "D"(a1), "S"(a2) : "rcx", "r11", "memory");
    return ret;
}

static long syscall3(long n, long a1, long a2, long a3) {
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n), "D"(a1), "S"(a2), "d"(a3) : "rcx", "r11", "memory");
    return ret;
}

static long syscall4(long n, long a1, long a2, long a3, long a4) {
    long ret;
    register long r10 __asm__("r10") = a4;
    __asm__ volatile("syscall"
        : "=a"(ret)
        : "a"(n), "D"(a1), "S"(a2), "d"(a3), "r"(r10)
        : "rcx", "r11", "r8", "memory");
    return ret;
}

static void print(const char *s) {
    int len = 0;
    while (s[len]) len++;
    syscall3(SYS_write, 1, (long)s, len);
}

static void print_hex(unsigned long n) {
    print("0x");
    if (n == 0) { print("0"); return; }
    char buf[16];
    int i = 0;
    while (n > 0) { int d = n % 16; buf[i++] = d < 10 ? '0' + d : 'a' + d - 10; n /= 16; }
    char rev[16];
    for (int j = 0; j < i; j++) rev[j] = buf[i - 1 - j];
    rev[i] = 0;
    print(rev);
}

#define AT_FDCWD (-100)
#define O_CREAT  0x40
#define O_RDWR   2

__attribute__((noreturn))
void _start(void) {
    const char *path = ".xbot/testfile";

    // Step 1: openat with O_CREAT|O_RDWR
    long fd = syscall4(SYS_openat, AT_FDCWD, (long)path, O_CREAT | O_RDWR, 0644);
    print("1. openat(");
    print(path);
    print(", O_CREAT|O_RDWR) = ");
    print_hex(fd);
    print("\n");

    if (fd < 0) {
        print("   FAIL\n");
        syscall1(SYS_exit, 1);
        __builtin_unreachable();
    }

    // Step 2: write
    long wn = syscall3(SYS_write, fd, (long)"hello", 5);
    print("2. write = ");
    print_hex(wn);
    print("\n");

    // Step 3: close
    syscall1(3, fd); // SYS_close=3

    // Step 4: stat
    char statbuf[144] __attribute__((aligned(16)));
    for (int i = 0; i < 144; i++) statbuf[i] = 0;
    long sr = syscall2(SYS_stat, (long)path, (long)statbuf);
    print("3. stat = ");
    print_hex(sr);
    print("\n");

    if (sr == 0) {
        long size = *(long *)(statbuf + 48);
        print("   st_size = ");
        print_hex(size);
        print("  OK!\n");
    } else {
        print("   FAIL!\n");
    }

    // Step 5: re-open without O_CREAT
    long fd2 = syscall4(SYS_openat, AT_FDCWD, (long)path, O_RDWR, 0);
    print("4. re-open = ");
    print_hex(fd2);
    print("\n");
    if (fd2 >= 0) syscall1(3, fd2);

    print("DONE\n");
    syscall1(SYS_exit, 0);
    __builtin_unreachable();
}

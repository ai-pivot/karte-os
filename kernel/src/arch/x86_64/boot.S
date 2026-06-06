# boot.S — Multiboot2 boot entry for x86_64 KarteOS
#
# GRUB loads us in 32-bit protected mode with paging ENABLED.
# We disable paging, set up identity mapping, enable long mode, and jump to 64-bit.
#
# Page table layout:
#   P4[0] → P3 → P2 (2MB pages, identity map 0..128MB for kernel + MMIO fallback)
#   P4[511] → P3_mmio → P2_mmio (map 0xFEE00000 area for LAPIC)
#   All mapped with 2MB huge pages for simplicity.

.set MULTIBOOT2_MAGIC, 0xE85250D6
.set MULTIBOOT2_ARCH,  0
.set MULTIBOOT2_HDR_SIZE, 24

.section .multiboot2, "a"
.balign 8
multiboot2_header:
    .long MULTIBOOT2_MAGIC
    .long MULTIBOOT2_ARCH
    .long MULTIBOOT2_HDR_SIZE
    .long 0xFFFFFFFF - MULTIBOOT2_MAGIC - MULTIBOOT2_ARCH - MULTIBOOT2_HDR_SIZE + 1
    .short 0
    .short 0
    .long 8

.section .data.boot, "aw", @progbits
.balign 4096
boot_p4_table:  .fill 4096, 1, 0
boot_p3_table:  .fill 4096, 1, 0
boot_p2_table:  .fill 4096, 1, 0
boot_stack:     .fill 4096 * 16, 1, 0
boot_stack_top:
.balign 4
gdt_desc:
    .short 0
    .long 0
far_ptr:
    .long 0
    .short 0

.section .text.boot
.code32
.global _start
.type _start, @function

.macro debug_char ch
    movw $0x3FD, %dx
1:  inb %dx, %al
    testb $0x20, %al
    jz 1b
    movw $0x3F8, %dx
    movb $'\ch', %al
    outb %al, %dx
.endm

_start:
    cli
    debug_char S

    movl %eax, %edi
    movl %ebx, %esi

    # ── Disable GRUB's paging ──
    movl %cr0, %eax
    andl $(~(1 << 31)), %eax
    movl %eax, %cr0
    xorl %eax, %eax
    movl %eax, %cr3

    # ── Get PC ──
    call .Lget_pc
.Lget_pc:
    popl %ebp

    # ── Set up identity-mapped page tables (0..128MB) ──
    # P4[0] → P3
    leal boot_p3_table - .Lget_pc(%ebp), %ecx
    orl $0x03, %ecx
    leal boot_p4_table - .Lget_pc(%ebp), %eax
    movl %ecx, (%eax)

    # P3[0] → P2
    leal boot_p2_table - .Lget_pc(%ebp), %ecx
    orl $0x03, %ecx
    leal boot_p3_table - .Lget_pc(%ebp), %eax
    movl %ecx, (%eax)

    # P2: 64 × 2MB huge pages covering 0..128MB
    leal boot_p2_table - .Lget_pc(%ebp), %edi
    movl $0, %ecx
    movl $64, %edx
.fill_loop:
    movl %ecx, %eax
    orl $0x00000083, %eax
    movl %eax, (%edi)
    movl $0, 4(%edi)
    addl $0x200000, %ecx
    addl $8, %edi
    decl %edx
    jnz .fill_loop

    # Also map P3[508..511] → same P2 entries to cover high MMIO area
    # LAPIC is at 0xFEE00000 = P4[510], P3[508..511], P2 offset
    # For simplicity, use 1GB page at P3[508] to map 0xFC0000000..0xFFFFFFFFF
    # But this is > 4GB physical which we don't have.
    # Instead, map P4[510] → P3 with 1:1 identity for MMIO range
    # Actually LAPIC 0xFEE00000 is at physical < 4GB, it's virtual 0xFEE00000 = P4[0] area.
    # 0xFEE00000 / 512GB = 0 → already covered by P4[0] → P3[0] → P2
    # Wait, 0xFEE00000 = 4,275,898,880 bytes = ~3.98GB
    # P4[0] covers 0..512GB, P3[0] covers 0..1GB. We need P3 entries for 1..4GB range.
    # Let's fill ALL 512 P3 entries (each covering 1GB via 512 P2 entries)
    # Actually, let's just fill P3 entries 1..3 with 1GB pages:

    # P3[1] = 1GB page at 1GB (0x40000000), present + writable + huge (1GB)
    leal boot_p3_table - .Lget_pc(%ebp), %edi
    movl $0x40000083, 8(%edi)       # P3[1]: 1GB at 1GB, PS=1
    movl $0, 12(%edi)

    # P3[2] = 1GB page at 2GB
    movl $0x80000083, 16(%edi)
    movl $0, 20(%edi)

    # P3[3] = 1GB page at 3GB (covers LAPIC at 0xFEE00000)
    movl $0xC0000083, 24(%edi)
    movl $0, 28(%edi)

    debug_char P

    # ── Enable PAE, OSFXSR (SSE), OSXMMEXCPT (SIMD exceptions) ──
    movl %cr4, %eax
    orl $((1 << 5) | (1 << 9) | (1 << 10)), %eax
    movl %eax, %cr4

    # ── Load CR3 ──
    leal boot_p4_table - .Lget_pc(%ebp), %eax
    movl %eax, %cr3

    # ── Enable long mode ──
    movl $0xC0000080, %ecx
    rdmsr
    orl $(1 << 8), %eax
    wrmsr

    # ── Load GDT ──
    leal trampoline_gdt - .Lget_pc(%ebp), %eax
    leal gdt_desc - .Lget_pc(%ebp), %edi
    movw $(trampoline_gdt_end - trampoline_gdt - 1), (%edi)
    movl %eax, 2(%edi)
    lgdt (%edi)

    debug_char G

    # ── Enable paging + clear CR0.EM/TS, set CR0.MP for SSE ──
    movl %cr0, %eax
    orl $((1 << 31) | (1 << 0) | (1 << 1)), %eax   # PG | PE | MP
    andl $~((1 << 2) | (1 << 3)), %eax               # clear EM, TS
    movl %eax, %cr0

    # ── Far jump to 64-bit code ──
    leal _start64 - .Lget_pc(%ebp), %eax
    leal far_ptr - .Lget_pc(%ebp), %edi
    movl %eax, (%edi)
    movw $0x08, 4(%edi)

    debug_char J

    ljmpl *(%edi)

.balign 16
trampoline_gdt:
    .quad 0x0000000000000000      # null
    .quad 0x00209A0000000000      # 64-bit code (0x08)
    .quad 0x0000920000000000      # data (0x10)
trampoline_gdt_end:

.code64
_start64:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %fs
    movw %ax, %gs
    movw %ax, %ss

    movw $0x3FD, %dx
1:  inb %dx, %al
    testb $0x20, %al
    jz 1b
    movw $0x3F8, %dx
    movb $'K', %al
    outb %al, %dx

    lea boot_stack_top(%rip), %rsp

    # Pass multiboot2 info to kmain:
    #   EDI = multiboot2 magic (from %eax at _start)
    #   ESI = multiboot2 info physical address (from %ebx at _start)
    # Clear upper 32 bits to ensure valid 64-bit values.
    movl %edi, %edi
    movl %esi, %esi

    call kmain

    cli
1:  hlt
    jmp 1b

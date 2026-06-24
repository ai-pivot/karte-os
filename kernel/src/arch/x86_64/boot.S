# boot.S — Multiboot2 boot entry for x86_64 KarteOS (high-half kernel)
#
# GRUB loads us in 32-bit protected mode with paging ENABLED at physical 1 MB.
# We disable GRUB's paging, set up our own page tables with BOTH identity mapping
# (low 4 GB) and a high-half direct map (DIRECT_MAP_BASE + phys), enable long mode,
# and jump to 64-bit code at the identity address.  Once in 64-bit mode we set up
# the stack and then jump to kmain at its high-half VMA.
#
# DIRECT_MAP_BASE = 0xFFFF_FFFF_8000_0000 → PML4[511], PDP[510].
#
# Page table layout (boot):
#   PML4[0]   → PDP_identity:
#     PDP[0]    → PD (2 MB pages, phys 0..512 MB)
#     PDP[1..3] = 1 GB pages (phys 1..4 GB)
#   PML4[511] → PDP_high:
#     PDP[510]  → same PD (2 MB pages, maps DIRECT_MAP_BASE+0..512MB → 0..512MB)
#     PDP[511]  = 1 GB page (maps DIRECT_MAP_BASE+1GB..2GB → 1GB..2GB)

.set MULTIBOOT2_MAGIC, 0xE85250D6
.set MULTIBOOT2_ARCH,  0
.set MULTIBOOT2_HDR_SIZE, 24

# PML4 index for the direct map: DIRECT_MAP_BASE = 0xFFFF_FFFF_8000_0000 → PML4[511]
.set DIRECT_MAP_PML4_IDX, 511

# PDP index within PML4[511]:
#   0xFFFF_FFFF_8000_0000 >> 30 = 0x3FFFFFFFE, & 0x1FF = 0x1FE = 510
.set DIRECT_MAP_PDP_IDX, 510

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
boot_p4_table:  .fill 4096, 1, 0     # PML4
boot_p3_table:  .fill 4096, 1, 0     # PDP for PML4[0]   (identity)
boot_p3_high:   .fill 4096, 1, 0     # PDP for PML4[511] (direct map)
boot_p2_table:  .fill 4096, 1, 0     # PD shared by both PDPs (2 MB pages 0..1 GB)
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

    # Save multiboot2 magic and info pointer to fixed low memory addresses
    # (safe area below the boot page tables at 0x10000)
    movl %eax, 0x8000
    movl %ebx, 0x8004

    # ── Disable GRUB's paging ──
    movl %cr0, %eax
    andl $(~(1 << 31)), %eax
    movl %eax, %cr0
    xorl %eax, %eax
    movl %eax, %cr3

    # ── Get PC (for PC-relative addressing in 32-bit mode) ──
    call .Lget_pc
.Lget_pc:
    popl %ebp

    # ════════════════════════════════════════════════════════════
    # Set up boot page tables: identity map + direct map
    # ════════════════════════════════════════════════════════════

    # ── PML4[0] → boot_p3_table (identity) ──
    leal boot_p3_table - .Lget_pc(%ebp), %ecx
    orl $0x03, %ecx              # PRESENT | WRITABLE
    leal boot_p4_table - .Lget_pc(%ebp), %eax
    movl %ecx, 0(%eax)           # PML4[0]
    movl $0, 4(%eax)

    # ── PML4[511] → boot_p3_high (direct map) ──
    leal boot_p3_high - .Lget_pc(%ebp), %ecx
    orl $0x03, %ecx
    leal boot_p4_table - .Lget_pc(%ebp), %eax
    movl %ecx, (DIRECT_MAP_PML4_IDX * 8)(%eax)   # PML4[511]
    movl $0, (DIRECT_MAP_PML4_IDX * 8 + 4)(%eax)

    # ── PDP_identity[0] → boot_p2_table (2 MB pages) ──
    leal boot_p2_table - .Lget_pc(%ebp), %ecx
    orl $0x03, %ecx
    leal boot_p3_table - .Lget_pc(%ebp), %eax
    movl %ecx, 0(%eax)           # PDP_identity[0]
    movl $0, 4(%eax)

    # ── PDP_high[510] → boot_p2_table (SAME PD, shared) ──
    # PDP_high is at index 510 (0x1FE) because DIRECT_MAP_BASE maps to PDP[510].
    leal boot_p3_high - .Lget_pc(%ebp), %eax
    movl %ecx, (DIRECT_MAP_PDP_IDX * 8)(%eax)    # PDP_high[510]
    movl $0, (DIRECT_MAP_PDP_IDX * 8 + 4)(%eax)

    # ── PD: 256 × 2 MB huge pages covering 0..512 MB ──
    leal boot_p2_table - .Lget_pc(%ebp), %edi
    movl $0, %ecx
    movl $256, %edx
.fill_loop:
    movl %ecx, %eax
    orl $0x00000083, %eax        # PRESENT | WRITABLE | PS (huge)
    movl %eax, (%edi)
    movl $0, 4(%edi)
    addl $0x200000, %ecx
    addl $8, %edi
    decl %edx
    jnz .fill_loop

    # ── PDP_identity[1..3] and PDP_high[511]: 1 GB pages ──
    # Identity: PDP_identity[1..3] map phys 1..4 GB at identity addresses
    # Direct:   PDP_high[511] maps phys 1..2 GB at DIRECT_MAP_BASE+1GB

    leal boot_p3_table - .Lget_pc(%ebp), %edi    # identity PDP

    # PDP_identity[1] = 1 GB page at 1 GB (0x40000000)
    movl $0x40000083, 8(%edi)        # PS=1, PRESENT, WRITABLE
    movl $0, 12(%edi)

    # PDP_identity[2] = 1 GB page at 2 GB (0x80000000)
    movl $0x80000083, 16(%edi)
    movl $0, 20(%edi)

    # PDP_identity[3] = 1 GB page at 3 GB (0xC0000000, covers LAPIC at 0xFEE00000)
    movl $0xC0000083, 24(%edi)
    movl $0, 28(%edi)

    # PDP_high[511] = 1 GB page at 1 GB
    leal boot_p3_high - .Lget_pc(%ebp), %esi     # high PDP
    movl $0x40000083, (511 * 8)(%esi)
    movl $0, (511 * 8 + 4)(%esi)

    debug_char P

    # ── Enable PAE, OSFXSR, OSXMMEXCPT ──
    movl %cr4, %eax
    orl $((1 << 5) | (1 << 9) | (1 << 10)), %eax
    andl $~(1 << 23), %eax             # CET disabled
    movl %eax, %cr4

    # ── Load CR3 ──
    leal boot_p4_table - .Lget_pc(%ebp), %eax
    movl %eax, %cr3

    # ── Enable long mode (EFER.LME) ──
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

    # ── Enable paging + clear CR0.EM/TS, set CR0.MP ──
    movl %cr0, %eax
    orl $((1 << 31) | (1 << 0) | (1 << 1)), %eax   # PG | PE | MP
    andl $~((1 << 2) | (1 << 3)), %eax               # clear EM, TS
    movl %eax, %cr0

    # ── Far jump to 64-bit code (at identity address) ──
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
.globl _start64
_start64:
    # When booted via EFI stub, UEFI's GDT is still active and valid.
    # No need to reload segment registers — UEFI's flat segments work fine.
    # The kernel's own GDT will be loaded later by kmain -> arch::init.

    # Direct COM1 output — no wait loop
    movw $0x3F8, %dx
    movb $'K', %al
    outb %al, %dx

    # Zero .bss section.
    # EFI stub loads a flat binary (objcopy -O binary) which has no ELF
    # headers — there is no loader to zero .bss for us.  QEMU memory
    # happens to be zero-initialised, but real UEFI firmware leaves
    # arbitrary data.  Un-zeroed .bss means random values for all
    # static mut / Atomic* / Mutex globals → unpredictable crashes.
    lea _sbss(%rip), %rdi
    lea _ebss(%rip), %rcx
    sub %rdi, %rcx
    shr $3, %rcx
    xor %eax, %eax
    rep stosq

    # Set up boot stack
    lea boot_stack_top(%rip), %rsp

    # Load multiboot2 magic and info pointer from fixed low memory
    movl 0x8000, %edi
    movl 0x8004, %esi
    movl %edi, %edi
    movl %esi, %esi

    # Jump to kmain at its high-half VMA
    movabs $kmain, %rax
    call *%rax

    cli
1:  hlt
    jmp 1b

    # kmain should never return, but just in case:
    cli
1:  hlt
    jmp 1b

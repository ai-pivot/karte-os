# boot.S — Multiboot2 boot entry for x86_64 KarteOS
#
# This is the ONLY file that requires hand-written assembly.
# It handles the transition from 32-bit protected mode (loaded by GRUB/Multiboot2)
# to 64-bit long mode, then calls the Rust kmain.
#
# Multiboot2 spec requirements:
#   - Header must be in the first 32768 bytes of the OS image
#   - Header must be 8-byte aligned
#   - EAX = 0x36D76289 (magic) and EBX = multiboot2 info struct address on entry
#
# Assembler: AT&T syntax for GNU AS (gas)

    .section .multiboot2, "a"
    .align 8
multiboot2_header_start:
    .long 0xE85250D6                                       # magic
    .long 0                                                 # architecture: 0 (i386 protected mode)
    .long multiboot2_header_end - multiboot2_header_start   # header length
    .long -(0xE85250D6 + 0 + (multiboot2_header_end - multiboot2_header_start)) # checksum

    # Framebuffer tag (optional — request text mode)
    .align 8
    .word 5                    # type = framebuffer
    .word 0                    # flags
    .long 20                   # size
    .long 80                   # width
    .long 25                   # height
    .long 0                    # depth (0 = text mode)

    # End tag
    .align 8
    .word 0                    # type = end
    .word 0                    # flags
    .long 8                    # size
multiboot2_header_end:

# ─────────────────────────────────────────────────────────────
# 32-bit entry point — loaded by Multiboot2 bootloader
# EAX = 0x36D76289 (multiboot2 magic), EBX = info struct address
# ─────────────────────────────────────────────────────────────
    .section .text.boot
    .code32
    .global _start
_start:
    # Save multiboot2 info pointer (EBX) in EDI for later
    movl %ebx, %edi

    # ── Step 1: Set up temporary page tables ──
    # Identity-map the first 128MB using 2MB pages.
    # PML4[0] → PDPT[0] → PD[0..63] with 2MB pages

    # Clear PML4 (4096 bytes)
    movl $pml4_table, %ecx
    xorl %eax, %eax
    movl $1024, %edx           # 1024 dword iterations = 4096 bytes
.clear_pml4:
    movl %eax, (%ecx)
    addl $4, %ecx
    decl %edx
    jnz .clear_pml4

    # Clear PDPT (4096 bytes)
    movl $pdpt_table, %ecx
    movl $1024, %edx
.clear_pdpt:
    movl %eax, (%ecx)
    addl $4, %ecx
    decl %edx
    jnz .clear_pdpt

    # Clear PD (8192 bytes = 2 pages)
    movl $pd_table, %ecx
    movl $2048, %edx
.clear_pd:
    movl %eax, (%ecx)
    addl $4, %ecx
    decl %edx
    jnz .clear_pd

    # PML4[0] → PDPT (present + writable)
    movl $pdpt_table, %eax
    orl $0x03, %eax            # present + writable
    movl %eax, pml4_table
    movl $0, pml4_table + 4    # high 32 bits = 0

    # PDPT[0] → PD (present + writable)
    movl $pd_table, %eax
    orl $0x03, %eax            # present + writable
    movl %eax, pdpt_table
    movl $0, pdpt_table + 4    # high 32 bits = 0

    # PD entries: 64 entries of 2MB pages (128MB total)
    # Each entry: flags(0x83) | phys_addr[20:13] in low word, 0 in high word
    # Entry format: [phys_addr_2M_aligned | flags]
    # flags = present(1) + writable(2) + page_size(0x80) = 0x83
    movl $pd_table, %ecx
    movl $0, %ebx              # physical address counter (increment by 2MB)
    movl $0, %esi              # entry counter
.fill_pd:
    # Low 32 bits: flags + low physical address bits
    movl %ebx, %eax
    orl $0x83, %eax            # present + writable + page_size
    movl %eax, (%ecx)
    # High 32 bits: high physical address bits (always 0 for <4GB)
    movl $0, 4(%ecx)
    addl $8, %ecx              # next PD entry
    addl $0x200000, %ebx       # next 2MB physical page
    incl %esi
    cmpl $64, %esi             # 64 entries × 2MB = 128MB
    jl .fill_pd

    # ── Step 2: Load PML4 into CR3 ──
    movl $pml4_table, %eax
    movl %eax, %cr3

    # ── Step 3: Enable PAE (CR4.PAE = bit 5) ──
    movl %cr4, %eax
    orl $(1 << 5), %eax
    movl %eax, %cr4

    # ── Step 4: Enable long mode (IA32_EFER.LME = bit 8) ──
    movl $0xC0000080, %ecx     # IA32_EFER MSR
    rdmsr
    orl $(1 << 8), %eax
    wrmsr

    # ── Step 5: Enable paging (CR0.PG = bit 31) ──
    movl %cr0, %eax
    orl $(1 << 31), %eax
    movl %eax, %cr0

    # ── Step 6: Load 64-bit GDT ──
    lgdt gdt64_pointer

    # ── Step 7: Far jump to 64-bit code ──
    ljmpl $0x08, $long_mode_entry

# ─────────────────────────────────────────────────────────────
# 64-bit long mode entry
# ─────────────────────────────────────────────────────────────
    .section .text.boot
    .code64
long_mode_entry:
    # Set up segment registers for kernel data segment (selector 0x10)
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %fs
    movw %ax, %gs
    movw %ax, %ss

    # Set up boot stack
    leaq boot_stack_top(%rip), %rsp

    # Clear BSS section
    leaq _sbss(%rip), %rdi
    leaq _ebss(%rip), %rcx
    subq %rdi, %rcx           # rcx = number of bytes to clear
    xorq %rax, %rax
    rep stosb

    # Call kmain(rdi = multiboot2 info addr, rsi = magic)
    # RDI already has the multiboot2 info pointer from EBX
    movl $0x36D76289, %esi     # pass multiboot2 magic as second arg
    call kmain

    # Halt if kmain returns (it shouldn't)
.halt:
    cli
    hlt
    jmp .halt

# ─────────────────────────────────────────────────────────────
# 64-bit GDT for long mode bootstrap
# ─────────────────────────────────────────────────────────────
    .section .rodata
    .align 16
gdt64:
    .quad 0x0000000000000000    # 0x00: null descriptor
    .quad 0x00209A0000000000    # 0x08: 64-bit kernel code (L, exec/read, DPL=0)
    .quad 0x0000920000000000    # 0x10: 64-bit kernel data (read/write, DPL=0)
    .quad 0x0000000000000000    # 0x18: reserved (TSS low — filled by Rust GDT init)
    .quad 0x0000000000000000    # 0x20: reserved (TSS high)
    .quad 0x0020FA0000000000    # 0x28: 64-bit user code (L, exec/read, DPL=3)
    .quad 0x0000F20000000000    # 0x30: 64-bit user data (read/write, DPL=3)
gdt64_end:

gdt64_pointer:
    .word gdt64_end - gdt64 - 1  # GDT limit
    .quad gdt64                    # GDT base address

# ─────────────────────────────────────────────────────────────
# BSS: page tables and boot stack
# ─────────────────────────────────────────────────────────────
    .section .bss
    .align 4096
pml4_table:
    .skip 4096
pdpt_table:
    .skip 4096
pd_table:
    .skip 4096 * 2               # 2 PD tables for up to 2GB mapping
boot_stack_bottom:
    .skip 4096 * 16              # 64KB boot stack
boot_stack_top:

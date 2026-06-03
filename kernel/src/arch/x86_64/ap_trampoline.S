# AP (Application Processor) trampoline code for x86_64 SMP boot.
#
# This code is copied to physical address 0x7000 at boot time.
# BSP sends INIT + SIPI IPIs to APs, which start executing here in 16-bit real mode.
# The trampoline transitions: real mode -> protected mode -> long mode -> Rust entry.
#
# Data area (written by BSP before sending SIPI):
#   trampoline_stack:  AP kernel stack pointer (8 bytes)
#   trampoline_entry:  Rust entry point (8 bytes)
#   trampoline_cr3:    Page table root (8 bytes)
#   trampoline_cpu_id: CPU ID (8 bytes)

.section .text
.global ap_trampoline_start
.global ap_trampoline_data
.global ap_trampoline_end
.type ap_trampoline_start, @function
.align 16

.code16
ap_trampoline_start:
    cli
    xor %ax, %ax
    mov %ax, %ds
    mov %ax, %es

    # Load temporary GDT (flat model)
    lgdt %cs:gdt64_desc - ap_trampoline_start

    # Enter protected mode
    mov %cr0, %eax
    or $1, %eax
    mov %eax, %cr0

    # Far jump to 32-bit protected mode code
    ljmp $0x08, $pm_entry - ap_trampoline_start

.code32
pm_entry:
    # Set up segment registers for protected mode
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %fs
    mov %ax, %gs
    mov %ax, %ss

    # Load page table (CR3)
    mov (trampoline_cr3 - ap_trampoline_start), %eax
    mov %eax, %cr3

    # Enable PAE (CR4.PAE = bit 5)
    mov %cr4, %eax
    or $(1 << 5), %eax
    mov %eax, %cr4

    # Enable long mode (EFER.LME = bit 8)
    mov $0xC0000080, %ecx
    rdmsr
    or $(1 << 8), %eax
    wrmsr

    # Enable paging (CR0.PG = bit 31)
    mov %cr0, %eax
    or $(1 << 31), %eax
    mov %eax, %cr0

    # Far jump to 64-bit long mode code
    ljmp $0x08, $lm_entry - ap_trampoline_start

.code64
lm_entry:
    # Load AP stack pointer
    mov (trampoline_stack - ap_trampoline_start), %rsp

    # Load Rust entry function address
    mov (trampoline_entry - ap_trampoline_start), %rax

    # Load CPU ID as first argument
    mov (trampoline_cpu_id - ap_trampoline_start), %rdi

    # Jump to Rust AP entry point
    jmp *%rax

    # Should never return
    hlt
    jmp lm_entry

.align 8
gdt64_start:
    .quad 0                    # Null descriptor
    .quad 0x00209A0000000000   # Code segment (64-bit, read/exec)
    .quad 0x0000920000000000   # Data segment (read/write)
gdt64_end:

gdt64_desc:
    .word gdt64_end - gdt64_start - 1  # GDT limit
    .long gdt64_start - ap_trampoline_start + 0x7000  # GDT base (physical)
    .long 0

.align 8
ap_trampoline_data:
trampoline_stack:
    .quad 0     # AP kernel stack pointer
trampoline_entry:
    .quad 0     # Rust entry function address
trampoline_cr3:
    .quad 0     # CR3 (page table root)
trampoline_cpu_id:
    .quad 0     # CPU ID (passed as rdi to entry function)

ap_trampoline_end:
    nop

.size ap_trampoline_start, . - ap_trampoline_start

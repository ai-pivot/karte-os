//! AP (Application Processor) trampoline code for x86_64 SMP boot.
//!
//! This code is copied to physical address 0x7000 at boot time.
//! BSP sends INIT + SIPI IPIs to APs, which start executing here in 16-bit real mode.
//! The trampoline transitions: real mode → protected mode → long mode → jump to Rust entry.
//!
//! Data area (written by BSP before sending SIPI):
//!   0x7000 + trampoline_data: AP stack pointer (8 bytes)
//!   0x7000 + trampoline_data + 8:  Rust entry point (8 bytes)
//!   0x7000 + trampoline_data + 16: CR3 value (8 bytes)
//!   0x7000 + trampoline_data + 24: CPU ID (8 bytes)

.section .text
.global ap_trampoline_start
.global ap_trampoline_data
.global ap_trampoline_end
.type ap_trampoline_start, @function
.align 16

.code16
ap_trampoline_start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax

    ; ── Load temporary GDT (flat model) ──
    lgdt [cs:gdt64_desc - ap_trampoline_start]

    ; ── Enter protected mode ──
    mov eax, cr0
    or  eax, 1
    mov cr0, eax

    ; Far jump to 32-bit protected mode code
    jmp 0x08:pm_entry - ap_trampoline_start

.code32
pm_entry:
    ; Set up segment registers for protected mode
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    ; ── Load page table (CR3) ──
    mov eax, [trampoline_cr3 - ap_trampoline_start]
    mov cr3, eax

    ; ── Enable PAE (CR4.PAE = bit 5) ──
    mov eax, cr4
    or  eax, (1 << 5)
    mov cr4, eax

    ; ── Enable long mode (EFER.LME = bit 8) ──
    mov ecx, 0xC0000080    ; IA32_EFER MSR
    rdmsr
    or  eax, (1 << 8)
    wrmsr

    ; ── Enable paging (CR0.PG = bit 31) ──
    mov eax, cr0
    or  eax, (1 << 31)
    mov cr0, eax

    ; Far jump to 64-bit long mode code
    jmp 0x08:lm_entry - ap_trampoline_start

.code64
lm_entry:
    ; Load AP stack pointer
    mov rsp, [trampoline_stack - ap_trampoline_start]

    ; Load Rust entry function address
    mov rax, [trampoline_entry - ap_trampoline_start]

    ; Load CPU ID as first argument
    mov rdi, [trampoline_cpu_id - ap_trampoline_start]

    ; Jump to Rust AP entry point
    jmp rax

    ; ── Should never return ──
    hlt
    jmp lm_entry

.align 8
gdt64_start:
    .quad 0                    ; Null descriptor
    .quad 0x00209A0000000000   ; Code segment (64-bit, read/exec)
    .quad 0x0000920000000000   ; Data segment (read/write)
gdt64_end:

gdt64_desc:
    .word gdt64_end - gdt64_start - 1  ; GDT limit
    .long gdt64_start - ap_trampoline_start + 0x7000  ; GDT base (physical)
    .long 0

.align 8
ap_trampoline_data:
trampoline_stack:
    .quad 0     ; AP kernel stack pointer
trampoline_entry:
    .quad 0     ; Rust entry function address
trampoline_cr3:
    .quad 0     ; CR3 (page table root)
trampoline_cpu_id:
    .quad 0     ; CPU ID (passed as rdi to entry function)

ap_trampoline_end:
    nop

.size ap_trampoline_start, . - ap_trampoline_start

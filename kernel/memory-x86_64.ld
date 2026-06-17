OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

/*
 * High-half kernel linker script for x86_64 KarteOS.
 *
 * GRUB loads the kernel at physical 1 MB.  The boot sections are linked at
 * VMA = 1 MB (identity) so GRUB can enter before paging is set up.  After
 * the boot code enables long-mode paging and creates a direct map, it jumps
 * to the high-half virtual address where the kernel proper lives.
 *
 * Critical invariant: every kernel section satisfies
 *   VMA = DIRECT_MAP_BASE + LMA
 *
 * This ensures the direct map (DIRECT_MAP_BASE + phys → phys) correctly
 * aliases every byte of kernel code, data, and BSS.  We enforce this by
 * computing each section's AT() (LMA) from its ADDR() (VMA):
 *   AT(ADDR(.section) - DIRECT_MAP_BASE)
 */

DIRECT_MAP_BASE = 0xFFFFFFFF80000000;

PHDRS {
    boot   PT_LOAD FLAGS(5);    /* R+X */
    text   PT_LOAD FLAGS(5);    /* R+X */
    rodata PT_LOAD FLAGS(4);    /* R   */
    data   PT_LOAD FLAGS(6);    /* RW  */
    bss    PT_LOAD FLAGS(6);
}

SECTIONS {
    . = 1M;

    /* ── Boot sections: linked AND loaded at 1 MB (identity-mapped) ── */
    .multiboot2 ALIGN(8) : {
        KEEP(*(.multiboot2))
    } :boot

    .text.boot : {
        *(.text.boot)
    } :boot

    .data.boot : {
        *(.data.boot)
    } :boot

    . = ALIGN(4096);
    _kernel_phys_start = .;       /* Physical address where kernel begins */

    /* ── Kernel sections: VMA = DIRECT_MAP_BASE + physical ──
     * Adding DIRECT_MAP_BASE to the location counter shifts the VMA into
     * the high half while the AT() directive maps each section back to
     * the correct physical address. */
    . += DIRECT_MAP_BASE;

    .text : AT(ADDR(.text) - DIRECT_MAP_BASE) {
        *(.text .text.*)
    } :text

    .rodata : AT(ADDR(.rodata) - DIRECT_MAP_BASE) {
        *(.rodata .rodata.*)
    } :rodata

    .data : AT(ADDR(.data) - DIRECT_MAP_BASE) {
        *(.data .data.*)
        *(.sdata .sdata.*)
    } :data

    .got : AT(ADDR(.got) - DIRECT_MAP_BASE) {
        *(.got .got.*)
    } :data

    .bss (NOLOAD) : AT(ADDR(.bss) - DIRECT_MAP_BASE) {
        _sbss = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        _ebss = .;
    } :bss

    . = ALIGN(4096);
    _ekernel = .;

    /DISCARD/ : {
        *(.dynsym)
        *(.dynstr)
        *(.gnu.hash)
        *(.hash)
        *(.dynamic)
        *(.rela.dyn)
        *(.note.*)
        *(.eh_frame*)
        *(.comment)
    }
}

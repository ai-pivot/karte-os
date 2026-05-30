OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

PHDRS {
    boot PT_LOAD FLAGS(5);    /* R+X */
    rodata PT_LOAD FLAGS(4);  /* R */
    data PT_LOAD FLAGS(6);    /* RW */
    bss PT_LOAD FLAGS(6);
}

SECTIONS {
    . = 1M;

    .multiboot2 ALIGN(8) : {
        KEEP(*(.multiboot2))
    } :boot

    .text : {
        *(.text.boot)
        *(.text .text.*)
    } :boot

    .data.boot : {
        *(.data.boot)
    } :boot

    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    .data : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    } :data

    .got : {
        *(.got .got.*)
    } :data

    .bss : {
        _sbss = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        _ebss = .;
    } :bss

    . = ALIGN(16);
    . += 4096 * 16;
    _boot_stack_top = .;

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

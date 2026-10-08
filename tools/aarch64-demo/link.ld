/* P3.1 aarch64 demo — QEMU virt: kernel ELF loaded at 0x40000000, EL1 */
ENTRY(_start)

SECTIONS
{
    . = 0x40100000;
    .text : { KEEP(*(.text.boot)) *(.text*) }
    .rodata : { *(.rodata*) }
    .data : { *(.data*) }
    .bss : { *(.bss*) *(COMMON) }

    /* 64 KiB boot stack in .bss for the boot hart */
    . = ALIGN(16);
    . = . + 0x10000;
    __boot_stack_top = .;
}

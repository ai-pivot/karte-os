PROVIDE(_skernel = 0x80200000);

MEMORY
{
    RAM (rwx) : ORIGIN = 0x80200000, LENGTH = 128M
}

SECTIONS
{
    .text : {
        *(.text.entry)
        *(.text .text.*)
    } > RAM

    .rodata : {
        *(.rodata .rodata.*)
    } > RAM

    .data : {
        *(.data .data.*)
    } > RAM

    .bss : {
        _sbss = .;
        *(.bss .bss.*)
        *(COMMON)
        _ebss = .;
    } > RAM

    . = ALIGN(16);
    . += 4096 * 4;
    _boot_stack_top = .;

    . = ALIGN(4096);
    _ekernel = .;
}

PROVIDE(_stext = ORIGIN(RAM));
PROVIDE(_etext = ADDR(.rodata));
PROVIDE(_srodata = ADDR(.rodata));
PROVIDE(_erodata = ADDR(.data));
PROVIDE(_sdata = ADDR(.data));
PROVIDE(_edata = ADDR(.bss));

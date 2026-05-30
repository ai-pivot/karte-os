ENTRY(_start)

SECTIONS {
    . = 1M;  /* Multiboot2 通常加载内核到 1MB */

    .multiboot2 ALIGN(8) : {
        *(.multiboot2)
    }

    .text : {
        *(.text.boot)
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    .data : {
        *(.data .data.*)
    }

    .bss : {
        _sbss = .;
        *(.bss .bss.*)
        *(COMMON)
        _ebss = .;
    }

    . = ALIGN(16);
    . += 4096 * 16;  /* 64KB boot stack */
    _boot_stack_top = .;

    . = ALIGN(4096);
    _ekernel = .;
}

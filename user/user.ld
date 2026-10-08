ENTRY(_start)

SECTIONS
{
    . = 0x00001000;

    .text : {
        *(.text .text.*)
        *(.rodata .rodata.*)
    }

    /* Data/bss start on a fresh page: keeps ELF segments page-disjoint so
       the loader never has to merge R/X and R/W permissions on one page
       (text's last page used to be shared with .bss; see process/mod.rs
       merge_page_flags for the union-permission fix that this complements). */
    . = ALIGN(4096);

    .data : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    .bss : {
        *(.bss .bss.*)
        *(.sbss .sbss.*)
    }

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}

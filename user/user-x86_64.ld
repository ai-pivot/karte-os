ENTRY(_start)

SECTIONS
{
    . = 0x1000;  /* 用户程序加载地址 */

    .text : {
        *(.text .text.*)
        *(.rodata .rodata.*)
    }

    .data : { *(.data) }
    .bss : { *(.bss) }

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}

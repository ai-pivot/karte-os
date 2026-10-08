OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY {
  RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 8M
}

SECTIONS {
  .text : {
    KEEP(*(.text.init))
    *(.text .text.*)
  } > RAM

  .rodata : ALIGN(4) { *(.rodata .rodata.*) *(.srodata .srodata.*) } > RAM
  .data   : ALIGN(4) { __global_pointer$ = . + 0x800; *(.data .data.* .sdata .sdata.*) } > RAM
  .bss    : ALIGN(4) { *(.bss .bss.* .sbss .sbss.*) *(COMMON) } > RAM

  . = ALIGN(16);
  . = . + 16K;
  _stack_top = .;

  /DISCARD/ : { *(.eh_frame .eh_frame_hdr) }
}

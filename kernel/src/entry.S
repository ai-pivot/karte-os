.section .text.entry
.global _start
_start:
    # OpenSBI passes hart_id in a0, dtb pointer in a1
    # Only let hart 0 proceed, park all other harts
    bnez    a0, .park

    # Set up stack pointer (use boot stack defined in linker script)
    la      sp, _boot_stack_top

    # Clear BSS section
    la      t0, _sbss
    la      t1, _ebss
.clear_bss:
    bgeu    t0, t1, .call_main
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       .clear_bss

.call_main:
    # Call kmain(hartid, dtb_ptr)
    # a0 = hartid (already set by OpenSBI)
    # a1 = dtb pointer (already set by OpenSBI)
    call    kmain

.park:
    wfi
    j       .park

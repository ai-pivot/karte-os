# KarteOS — AGENTS.md

> A modern RISC-V 64-bit operating system written in Rust 2024 Edition.

## Quick Start

```bash
make build          # Build kernel (cargo build --release)
make run            # Run in QEMU (single core)
make debug          # Run with GDB stub (-S -s)
make clean          # Clean all artifacts
```

QEMU exit: `Ctrl+A` then `X`.

## Build Requirements

- Rust stable (1.93+), target `riscv64gc-unknown-none-elf`
- `qemu-system-riscv64` (8.2+)
- `gcc-riscv64-linux-gnu` (for objdump/nm)
- Dependencies: `riscv`, `sbi-rt` (legacy feature), `buddy_system_allocator`, `virtio-drivers` (alloc feature), `spin`, `bitflags`

## Architecture Overview

S-mode kernel on OpenSBI (M-mode). Identity-mapped Sv39 virtual memory. 10-phase init in `kmain()` (main.rs). Trap handling via Direct-mode stvec with full register save/restore (trap.rs). Round-Robin scheduler with `__switch()` assembly context switch (switch.S). SMP via SBI `hart_start` for secondary harts.

Boot flow: QEMU → OpenSBI → `_start` (entry.S: BSS clear, stack setup) → `kmain` → init phases → `wfi` loop.

## GOTCHAS

- **Rust 2024 Edition**: `#[no_mangle]` → `#[unsafe(no_mangle)]`, `extern "C"` → `unsafe extern "C"`, `static mut` → use atomics/Mutex. Violating these causes confusing errors.
- **`console_println!` macro** is `#[macro_export]` — call as `crate::console_println!`, NOT `sbi::console_println!`.
- **sbi-rt legacy**: `sbi_rt::legacy::console_putchar()` requires `features = ["legacy"]` in Cargo.toml.
- **QEMU boot hart**: With `-smp N`, OpenSBI may boot on hart 1 (not always hart 0). entry.S must accept any hart.
- **VirtIO MMIO**: Devices at 0x10001000+ with 0x200 stride. QEMU without `-device virtio-blk-device` etc. has no devices — probe must handle LoadFault gracefully (trap handler skips faulting instruction).
- **PLIC address range**: Needs mapping 0x0C000000–0x0C400000 (multiple pages), not just one page.
- **amoswap/lr/sc**: RISC-V atomic extensions NOT available on bare `riscv64gc-unknown-none-elf` target — don't use inline atomics in assembly.
- **Kernel stack**: Boot stack defined in memory.x as `_boot_stack_top`. Task stacks are separately allocated via PMM.

## Knowledge Files

| File | Description |
|------|-------------|
| `docs/agent/architecture.md` | System architecture, boot flow, memory layout, subsystem relationships |
| `docs/agent/drivers.md` | UART, VirtIO block, VirtIO net, filesystem driver details and MMIO addresses |
| `docs/agent/memory.md` | PMM bitmap allocator, Sv39 VMM, heap allocator, page table entry flags |
| `docs/agent/scheduler.md` | Task structures, context switch assembly, Round-Robin algorithm |
| `docs/agent/trap.md` | Trap frame layout, exception dispatch, timer interrupts, syscall handling |
| `docs/agent/smp.md` | SMP hart management, BSP/secondary init, SBI hart_start |
| `docs/agent/conventions.md` | Rust 2024 patterns, coding style, error handling |

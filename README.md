# KarteOS

A modern RISC-V operating system written in Rust.

## Features

- **RISC-V 64-bit** (RV64GC) support, running on QEMU `virt` machine
- **Rust 2024 Edition** with modern language features
- **S-mode kernel** running on top of OpenSBI
- **Sv39 virtual memory** with 3-level page tables
- **Physical memory manager** using bitmap frame allocator
- **Kernel heap** via buddy system allocator
- **Trap handling** framework with timer interrupts
- **PLIC driver** for external interrupt control
- **UART driver** (ns16550a MMIO)
- **Round-Robin scheduler** with multiple demo tasks

## Prerequisites

```bash
# Install Rust RISC-V target
rustup target add riscv64gc-unknown-none-elf

# Install QEMU
sudo apt install qemu-system-misc
```

## Build & Run

```bash
# Build
make build

# Run in QEMU
make run

# Debug with GDB
make debug
```

## Architecture

```
┌──────────────────────────────────────┐
│         User Applications            │
├──────────────────────────────────────┤
│     System Call Interface (ecall)    │
├──────────┬──────────┬────────────────┤
│ Process  │ Filesystem│   Drivers     │
│ Manager  │  (VFS)   │  (VirtIO)     │
├──────────┴──────────┴────────────────┤
│      Scheduler / Executor            │
├──────────────────────────────────────┤
│    Virtual Memory (Sv39)             │
├──────────────────────────────────────┤
│    Physical Memory Manager (Buddy)   │
├──────────────────────────────────────┤
│    Sync Primitives (SpinLock)        │
├──────────────────────────────────────┤
│    Arch Layer (Trap, PLIC, Timer)    │
├──────────────────────────────────────┤
│    HAL: SBI / UART (MMIO)           │
├──────────────────────────────────────┤
│    OpenSBI (M-mode)                  │
└──────────────────────────────────────┘
```

## Project Structure

```
kernel/src/
├── main.rs           # Entry point and initialization
├── entry.S           # Boot assembly (BSS clear, stack setup)
├── lang_items.rs     # Panic handler
├── sbi.rs            # SBI console and system control
├── arch/
│   ├── trap.rs       # Trap handling framework
│   └── plic.rs       # PLIC interrupt controller
├── driver/
│   └── uart.rs       # ns16550a UART driver
├── mm/
│   ├── pmm.rs        # Physical frame allocator (bitmap)
│   ├── vmm.rs        # Sv39 virtual memory manager
│   └── heap.rs       # Kernel heap allocator
├── sync/
│   └── spinlock.rs   # Kernel spinlock
└── sched/
    ├── mod.rs        # Round-Robin scheduler
    └── task.rs       # Task control block
```

## Exit QEMU

Press `Ctrl+A` then `X` to exit QEMU.

## License

MIT

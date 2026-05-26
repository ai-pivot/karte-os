# KarteOS

<p align="center">
  <strong>A modern RISC-V 64-bit operating system written in Rust</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/target-riscv64gc-blue" />
  <img src="https://img.shields.io/badge/edition-2024-orange" />
  <img src="https://img.shields.io/badge/platform-QEMU%20virt-green" />
  <img src="https://img.shields.io/badge/tests-50%20passed-brightgreen" />
  <img src="https://img.shields.io/badge/license-MIT-informational" />
</p>

<p align="center">
  <img src="https://github.com/ai-pivot/karte-os/actions/workflows/ci.yml/badge.svg" alt="CI" />
</p>

---

## Overview

KarteOS is a from-scratch operating system targeting **RISC-V 64-bit** (RV64GC), built entirely in Rust 2024 Edition. It runs on QEMU's `virt` machine and leverages OpenSBI as the M-mode firmware layer.

**2512+ lines** of Rust and RISC-V assembly implementing a full OS stack: boot, memory management, virtual memory, trap handling, device drivers, filesystem, networking, system calls, multi-tasking, and SMP multi-core support.

## Features

### Core
- **RISC-V 64-bit** (RV64GC) — runs on QEMU `virt` machine with OpenSBI
- **Rust 2024 Edition** — uses modern `#[unsafe(no_mangle)]`, `unsafe extern`, atomic types
- **S-mode kernel** — runs in Supervisor mode on top of OpenSBI (M-mode)
- **Sv39 virtual memory** — 3-level page tables with identity mapping

### Memory Management
- **Physical frame allocator** — bitmap-based, manages 127 MB of RAM
- **Kernel heap** — buddy system allocator, 1 MB initial heap
- **Page table management** — map/unmap, identity mapping, MMIO mapping

### Hardware Support
- **UART driver** — ns16550a MMIO serial console
- **PLIC driver** — Platform-Level Interrupt Controller (per-hart enable/claim/complete)
- **Trap framework** — full context save/restore, exception dispatch, timer interrupts
- **VirtIO block device** — MMIO transport, DMA-compatible, probe & block read/write
- **VirtIO network** — VirtQueue management, MAC address, packet send/recv

### OS Services
- **System calls** — `write`, `exit`, `yield`, `getpid` via `ecall` (UserEnvCall)
- **Round-Robin scheduler** — context-switching with callee-saved register save/restore
- **In-memory filesystem** — create, read, write, delete, list operations
- **SMP multi-core** — BSP init + secondary hart startup via SBI `hart_start`
- **SpinLock** — kernel synchronization primitive with RAII guard

## Architecture

```
┌─────────────────────────────────────────────────┐
│              User Applications (U-mode)          │
├─────────────────────────────────────────────────┤
│        System Call Interface (ecall)             │
├───────────┬──────────────┬───────────────────────┤
│ Scheduler │  Filesystem  │    Device Drivers     │
│ (RR + CS) │  (in-memory) │  UART / VirtIO        │
├───────────┴──────────────┴───────────────────────┤
│           SMP Multi-Core Manager                  │
├──────────────────────────────────────────────────┤
│          Virtual Memory Manager (Sv39)           │
├──────────────────────────────────────────────────┤
│        Physical Memory Manager (bitmap)          │
├──────────────────────────────────────────────────┤
│         Sync Primitives (SpinLock<T>)            │
├──────────────────────────────────────────────────┤
│     Arch Layer (Trap, PLIC, Timer, SMP)          │
├──────────────────────────────────────────────────┤
│           HAL: SBI / UART (MMIO)                 │
├──────────────────────────────────────────────────┤
│              OpenSBI (M-mode)                     │
└──────────────────────────────────────────────────┘
```

## Quick Start

### Prerequisites

```bash
# Install Rust RISC-V target
rustup target add riscv64gc-unknown-none-elf

# Install QEMU and cross-compiler
sudo apt install qemu-system-misc gcc-riscv64-linux-gnu
```

### Build & Run

```bash
git clone https://github.com/ai-pivot/karte-os.git
cd karte-os

# Build
make build

# Run in QEMU (single core)
make run

# Run with 4 cores (SMP)
qemu-system-riscv64 -machine virt -cpu rv64 -nographic \
  -bios default -m 128M -smp 4 \
  -kernel target/riscv64gc-unknown-none-elf/release/karte-os-kernel
```

### Expected Output

```
OpenSBI v1.3.1 ...
...
=== KarteOS v0.2.0 ===
  Booting on hart 0
  DTB pointer: 0x87e00000
[init] Setting up trap handling...
[smp] BSP (hart 0) initialized
[init] Initializing physical memory...
[pmm] Initialized: 127 MB available
[init] Setting up virtual memory...
[vmm] Sv39 page table activated at 0x8021b000
[init] Initializing kernel heap...
[heap] Initialized: 1024 KB heap at 0x80261000
[init] Probing VirtIO devices...
[init] Initializing filesystem...
[fs] In-memory file system initialized
[init] Probing network devices...
[init] Enabling timer interrupts...
[init] Initializing PLIC...
[init] Starting secondary harts...
[init] Initializing scheduler...
[sched] Scheduler initialized with 3 tasks (context-switching mode)
=== KarteOS initialized successfully ===
[timer] tick 100 (1s)
[timer] tick 200 (2s)
...
```

### Exit QEMU

Press `Ctrl+A` then `X`.

## Testing

KarteOS includes **50 integration tests** that run inside QEMU as a specialized test kernel.

```bash
# Run all tests
make test

# Build test kernel only
make build-test

# Boot test (normal mode)
make boot-test
```

### Test Coverage

| Module | Tests | Coverage |
|--------|-------|----------|
| **PMM** (Physical Memory) | 6 | alloc, dealloc, cycle, uniqueness, reuse, alignment |
| **VMM** (Virtual Memory) | 6 | page table creation, map, identity map, PTE flags/PPN/leaf |
| **Heap** (Allocator) | 6 | Vec, String, large alloc, Box, multiple allocs, drop/realloc |
| **Filesystem** | 15 | CRUD, append, overwrite, duplicates, edge cases |
| **SpinLock** | 5 | lock/unlock, modify, guard drop, complex data, sequential |
| **Task** | 6 | context zeroed, goto, TCB creation, state transitions |
| **Syscall** | 6 | dispatch, getpid, write fd, yield, constants |
| **Total** | **50** | 7 modules fully tested |

### How It Works

1. `--features test_mode` compiles a test kernel with `run_tests()` in each module
2. QEMU boots the test kernel → runs all tests → prints TAP output → shuts down
3. `scripts/run-tests.sh` parses output and reports pass/fail

### CI

Every push and PR triggers GitHub Actions:
- **Build** — `cargo build --release` for riscv64gc target
- **Lint** — `cargo fmt --check` + `cargo clippy`
- **Test** — Build test kernel + run in QEMU, verify all 50 pass
- **Boot Test** — Normal mode boot, verify init sequence completes
- **SMP Test** — 4-core boot, verify multi-hart init

## Project Structure

```
karte-os/
├── Cargo.toml                 # Workspace root
├── Makefile                    # Build & run targets
├── .cargo/config.toml          # Target and linker config
├── rust-toolchain.toml         # Toolchain specification
├── kernel/
│   ├── Cargo.toml              # Kernel crate dependencies
│   ├── memory.x                # Linker script (0x80200000, 128MB RAM)
│   ├── build.rs                # Build script (linker search path)
│   └── src/
│       ├── main.rs             # #[entry] kmain — 10-phase init
│       ├── entry.S             # Boot: BSS clear, stack setup
│       ├── lang_items.rs       # Panic handler
│       ├── sbi.rs              # SBI console + shutdown
│       ├── arch/
│       │   ├── trap.rs         # Trap context + dispatch + timer
│       │   ├── plic.rs         # PLIC interrupt controller
│       │   └── smp.rs          # SMP multi-core management
│       ├── driver/
│       │   ├── uart.rs         # ns16550a UART (MMIO)
│       │   ├── virtio.rs       # VirtIO block device (MMIO)
│       │   ├── net.rs          # VirtIO network (MMIO)
│       │   └── fs.rs           # In-memory filesystem
│       ├── mm/
│       │   ├── pmm.rs          # Bitmap physical frame allocator
│       │   ├── vmm.rs          # Sv39 page tables
│       │   └── heap.rs         # Buddy system kernel heap
│       ├── sync/
│       │   └── spinlock.rs     # SpinLock<T> + RAII guard
│       ├── sched/
│       │   ├── mod.rs          # Round-Robin scheduler
│       │   ├── task.rs         # TaskContext + TaskControlBlock
│       │   └── switch.S        # Context switch (callee-saved regs)
│       └── syscall/
│           └── mod.rs          # Syscall dispatch (write/exit/yield/getpid)
└── docs/
    ├── plan-riscv-os.md         # Development plan
    └── agent/                   # Knowledge base (for AI-assisted dev)
```

## Boot Sequence

```
QEMU → OpenSBI (M-mode) → _start (entry.S) → kmain (main.rs)
                                    │
                                    ├── 1. UART + SBI console
                                    ├── 2. Trap vector setup
                                    ├── 3. SMP BSP init
                                    ├── 4. Physical memory (PMM)
                                    ├── 5. Virtual memory (Sv39)
                                    ├── 6. Kernel heap
                                    ├── 7. VirtIO device probe
                                    ├── 8. Filesystem init
                                    ├── 9. Network probe
                                    ├── 10. Timer + PLIC + Scheduler
                                    └── Enable interrupts → idle loop
```

## Key Technical Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Firmware | OpenSBI (QEMU default) | No custom M-mode code needed |
| Memory model | Sv39 | Standard RISC-V, well-supported |
| Frame allocator | Bitmap | Simple, sufficient for personal OS |
| Heap allocator | buddy_system_allocator crate | Mature, no_std compatible |
| Scheduler | Round-Robin + timer preemption | Simple, deterministic |
| VirtIO transport | MMIO | QEMU virt standard |
| Edition | Rust 2024 | Latest safety features |

## Dependencies

| Crate | Purpose |
|-------|---------|
| `riscv` | CSR access, register definitions |
| `sbi-rt` | SBI ecall wrappers (legacy + standard) |
| `buddy_system_allocator` | Kernel heap allocator |
| `virtio-drivers` | VirtIO device drivers |
| `spin` | Mutex primitives |
| `bitflags` | Page table entry flags |

## Development

KarteOS was developed using a **multi-agent parallel workflow**:
- Phase 1: 3 agents in parallel (scheduler, VirtIO block+FS, VirtIO net)
- Phase 2: 2 agents in parallel (syscalls, SMP)
- Integration + verification between phases

## Roadmap

- [ ] User-mode process loader (ELF)
- [ ] Virtual file system (VFS) layer
- [ ] VirtIO GPU / framebuffer console
- [ ] TCP/IP network stack (smoltcp)
- [ ] Complete POSIX-like syscall interface
- [ ] Real hardware support (SiFive, StarFive)

## License

MIT

# KarteOS

<p align="center">
  <strong>A modern dual-architecture (RISC-V 64 + x86_64) operating system written in Rust</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/target-riscv64gc-blue" />
  <img src="https://img.shields.io/badge/target-x86__64-red" />
  <img src="https://img.shields.io/badge/edition-2024-orange" />
  <img src="https://img.shields.io/badge/platform-QEMU%20virt-green" />
  <img src="https://img.shields.io/badge/tests-199%20total-brightgreen" />
  <img src="https://img.shields.io/badge/license-MIT-informational" />
</p>

<p align="center">
  <img src="https://github.com/ai-pivot/karte-os/actions/workflows/ci.yml/badge.svg" alt="CI" />
</p>

---

## Overview

KarteOS is a from-scratch operating system targeting **RISC-V 64-bit** (RV64GC) and **x86_64**, built entirely in Rust 2024 Edition. It runs on QEMU and supports both OpenSBI (RISC-V M-mode firmware) and direct GRUB boot (x86_64).

**33000+ lines** of Rust and assembly implementing a full OS stack: dual-architecture boot, memory management (bitmap PMM + Sv39/x86_64 paging), virtual memory with lazy allocation, trap handling, device drivers (UART, VirtIO, AHCI/SATA, NVMe, PS/2, VGA), ext4/FAT32 filesystems, smoltcp TCP/IP network stack, pipe IPC, 60+ system calls, multi-tasking with Round-Robin scheduler, fork/clone, epoll/eventfd/timerfd, Linux syscall compatibility layer (Go runtime support), and SMP multi-core support.

## Features

### Dual Architecture
- **RISC-V 64-bit** (RV64GC) — runs on QEMU `virt` machine with OpenSBI
- **x86_64** — runs on QEMU with GRUB ISO boot (`int 0x80` + `syscall` instruction)
- **Rust 2024 Edition** — `#[unsafe(no_mangle)]`, `unsafe extern`, atomic types
- Architecture-specific code under `arch/` with `#[cfg(target_arch)]` conditional compilation

### Memory Management
- **Physical frame allocator** — bitmap-based, supports dynamic RAM sizing (multiboot2 on x86_64)
- **Sv39 virtual memory** (RISC-V) — 3-level page tables with identity mapping, per-process address space
- **x86_64 paging** — 4-level (PML4) with 2MB/1GB huge pages, per-process CR3 isolation
- **Kernel heap** — buddy system allocator + linked_list_allocator
- **VMA tracking** — lazy allocation with page fault handler, `mmap`/`mprotect`/`madvise` support
- **Copy-on-write ready** — `fork()` deep-copies page tables (COW infrastructure in place)

### Hardware Support
- **UART driver** — ns16550a MMIO (RISC-V) + COM1 I/O ports (x86_64)
- **PLIC driver** (RISC-V) — per-hart enable/claim/complete
- **LAPIC + IOAPIC** (x86_64) — local APIC timer, I/O APIC interrupt routing
- **VirtIO block device** — MMIO transport (RISC-V), PCI→MMIO (x86_64)
- **AHCI/SATA** (x86_64) — PCI class 0x01/0x06/0x01, BAR5 MMIO, DMA engine
- **NVMe** (x86_64) — PCI class 0x01/0x08/0x02, BAR0 MMIO
- **VirtIO network** — VirtQueue management, MAC address, packet send/recv
- **VGA text mode** (x86_64) — 80×25 at 0xB8000, dual-output with serial
- **PS/2 keyboard** (x86_64) — IRQ 1, scancode Set 1, US layout

### Filesystems
- **ext4** (preferred) — vendored `ext4_rs` with sector write-through cache, multi-level paths
- **FAT32** (fallback) — `starry-fatfs` with LFN support
- **RamFS** (embedded) — ELF files compiled into kernel via `include_bytes!`
- **VFS layer** — unified fd table with `FdType` enum (File, Pipe, Socket, Stdio, TTY)
- Boot priority: ext4 → FAT32 → RamFS-only

### Network Stack
- **smoltcp 0.12** TCP/IP stack — full TCP/UDP/ICMP socket support
- Socket syscalls: `socket`, `bind`, `connect`, `listen`, `accept`, `sendto`, `recvfrom`, `shutdown`
- Timer-driven polling at ~10ms interval via kernel timer ISR
- QEMU user-mode networking (10.0.2.15/24)

### OS Services
- **60+ system calls** — file I/O, process management, memory, networking, IPC, terminal control
- **Round-Robin scheduler** — preemptive timer-interrupt-driven, `__switch()` context switch with FPU/SSE save
- **Multi-process** — `fork()` (deep copy), `clone()` (Linux ABI with TLS), `exec`/`spawn`, `waitpid`
- **Pipe IPC** — anonymous pipes with 4KB ring buffers, blocking read/write with scheduler integration
- **epoll / eventfd / timerfd** — Linux-compatible event notification
- **Linux syscall compatibility** — translates Linux numbers to native KarteOS calls (supports Go runtime)
- **Environment variables** — `setenv`/`getenv`, `CWD` tracking, `PATH` search for `exec`
- **TTY subsystem** — line editing, command history, raw/cooked mode via `ioctl` (TCSETS)
- **Signals** — `kill()` with SIGINT/SIGKILL/SIGTERM
- **SMP multi-core** — BSP + secondary cores via SBI `hart_start` (RISC-V) or LAPIC INIT/SIPI (x86_64)

### Synchronization
- **SpinLock** — short critical sections (RAII guard)
- **IntSpinLock** — like SpinLock but saves/restores interrupt enable
- **YieldMutex / BlockingMutex** — I/O-bound operations, yields scheduler instead of spinning
- **Atomic operations** — lock-free where possible (kernel log buffer, CR3 tracking)

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                   User Applications (U-mode / Ring 3)             │
│         Shell | cat | ls | grep | sed | wc | Go binaries         │
├──────────────────────────────────────────────────────────────────┤
│    System Call Interface (ecall / int 0x80 / syscall)             │
├────────────┬──────────────┬──────────────┬───────────────────────┤
│  Scheduler │  Filesystems │   Network    │    Device Drivers     │
│  (RR+CS)   │  ext4/FAT32/ │  smoltcp     │  UART/VirtIO/AHCI/    │
│            │  RamFS/VFS   │  TCP/IP      │  NVMe/VGA/PS2         │
├────────────┴──────────────┴──────────────┴───────────────────────┤
│                     IPC: Pipe | epoll | eventfd | timerfd         │
├──────────────────────────────────────────────────────────────────┤
│                Process Manager (fork/clone/exec/spawn)            │
├──────────────────────────────────────────────────────────────────┤
│              SMP Multi-Core Manager (BSP + secondaries)           │
├──────────────────────────────────────────────────────────────────┤
│               Virtual Memory Manager (Sv39 / x86_64 PML4)        │
│                    VMA tracking + lazy allocation                 │
├──────────────────────────────────────────────────────────────────┤
│             Physical Memory Manager (bitmap allocator)            │
├──────────────────────────────────────────────────────────────────┤
│          Sync Primitives (SpinLock / IntSpinLock / Mutex)         │
├──────────────────────────────────────────────────────────────────┤
│     Arch Layer (Trap, PLIC/LAPIC, Timer, SMP, GDT/IDT)           │
├──────────────────────────────────────────────────────────────────┤
│         HAL: SBI / UART MMIO / PCI / I/O Ports                    │
├──────────────────────────────────────────────────────────────────┤
│         Firmware: OpenSBI (RISC-V) / GRUB + Multiboot2 (x86_64)  │
└──────────────────────────────────────────────────────────────────┘
```

## Quick Start

### RISC-V 64 (primary)

```bash
# Install prerequisites
rustup target add riscv64gc-unknown-none-elf
sudo apt install qemu-system-misc gcc-riscv64-linux-gnu

# Clone and build
git clone https://github.com/ai-pivot/karte-os.git
cd karte-os

# Build user programs (required before kernel!)
cd user && make && cd ..

# Build, deploy, and run — ONE COMMAND
make shell

# Or step by step:
make                # Build & run
make test           # Run integration tests (96 tests)
```

### x86_64 (secondary)

```bash
# Install prerequisites
rustup toolchain install nightly
rustup target add x86_64-unknown-none
sudo apt install qemu-system-x86 grub-common xorriso

# Clone and build
git clone https://github.com/ai-pivot/karte-os.git
cd karte-os

# Build user programs for x86_64
cd user && make ARCH=x86_64 && cd ..

# Build, deploy, and run — ONE COMMAND
make shell-x86

# Or step by step:
make iso-x86        # Build ISO + deploy all programs
make test-x86       # Run integration tests (103 tests)
```

### Expected Output (RISC-V)

```
OpenSBI v1.3.1 ...
...
=== KarteOS v0.2.0 ===
  Booting on hart 0
[init] Initializing SMP...
[init] Probing VirtIO devices...
[init] Initializing filesystem...
[init] Initializing virtual filesystem...
[init] Initializing environment...
[init] Initializing PLIC...
[init] Initializing TTY...
[init] Starting secondary harts...
[init] Initializing scheduler...
[init] Loading user program...
[init] Initializing network...
[init] Entering user mode...
=== KarteOS initialized successfully ===
KarteOS Shell>
```

### Exit QEMU

Press `Ctrl+A` then `X`.

## Testing

KarteOS includes **199 integration tests** that run inside QEMU as a specialized test kernel:

| Architecture | Tests | Command |
|-------------|-------|---------|
| RISC-V 64   | 96    | `make test` |
| x86_64      | 103   | `make test-x86` |
| **Total**   | **199** | `make test-all` |

```bash
# Run all RISC-V tests
make test

# Run all x86_64 tests
make test-x86

# Run both architectures
make test-all

# Boot test (verify boot reaches shell)
make boot-test

# SMP test (4-core)
make smp-test
```

### Test Coverage (RISC-V Core)

| Module | Tests | Description |
|--------|-------|-------------|
| **PMM** (Physical Memory) | 6 | alloc, dealloc, cycle, uniqueness, reuse, alignment |
| **VMM** (Virtual Memory) | 19 | page table, map, identity map, PTE flags, unmap, permissions |
| **Heap** (Allocator) | 6 | Vec, String, large alloc, Box, multiple allocs, drop/realloc |
| **Filesystem** | 15 | CRUD, append, overwrite, duplicates, edge cases |
| **SpinLock** | 5 | lock/unlock, modify, guard drop, complex data, sequential |
| **IntSpinLock** | 5 | interrupt-safe lock/unlock, nested, reentrant |
| **Mutex** | 6 | blocking lock/unlock, contention, yield |
| **Task** | 5 | context zeroed, TCB creation, state transitions, scheduling |
| **Syscall** | 31 | dispatch, file I/O, mmap, brk, pipe, fork, epoll |
| **Total** | **96** | + arch-specific (RISC-V: 15, x86_64: 32) |

### How It Works

1. `--features test_mode` compiles a test kernel with `run_tests()` in each module
2. QEMU boots the test kernel → runs all tests → prints TAP output → shuts down
3. `scripts/run-tests.sh` parses output and reports pass/fail

### CI

Every push and PR triggers GitHub Actions (5 jobs):
- **Build** — `cargo build --release` + user programs
- **Lint** — `cargo fmt --check` + `cargo clippy`
- **Test** — Build test kernel + run in QEMU, verify all pass
- **Boot Test** — Normal mode boot, verify shell prompt
- **SMP Test** — Multi-core boot, verify multi-hart init

## Project Structure

```
karte-os/
├── Cargo.toml                     # Workspace root
├── Makefile                       # Build, run, test, deploy targets
├── .cargo/config.toml             # Target and linker config
├── rust-toolchain.toml            # Toolchain specification
├── kernel/
│   ├── Cargo.toml                 # Kernel crate dependencies
│   ├── build.rs                   # Build script (linker search path)
│   └── src/
│       ├── main.rs                # kmain: multi-phase init, test/normal dispatch
│       ├── platform.rs            # Architecture-gated platform abstractions
│       ├── lang_items.rs          # Panic handler
│       ├── kernel_log.rs          # Lock-free ring buffer logger
│       ├── env.rs                 # Environment variables (CWD, PATH)
│       ├── test.rs                # TAP test framework
│       ├── arch/
│       │   ├── mod.rs             # Arch dispatcher
│       │   ├── riscv64/
│       │   │   ├── entry.S        # Boot entry (BSS clear, stack setup)
│       │   │   ├── trap.rs        # Trap context, dispatch, timer, syscall
│       │   │   ├── plic.rs        # PLIC interrupt controller
│       │   │   ├── sbi.rs         # SBI console + shutdown
│       │   │   ├── smp.rs         # SMP hart management
│       │   │   ├── platform.rs    # RISC-V platform constants
│       │   │   └── test.rs        # RISC-V arch tests
│       │   └── x86_64/
│       │       ├── boot.rs        # Multiboot2 header + long mode entry
│       │       ├── gdt.rs         # GDT (64-bit code/data, TSS)
│       │       ├── idt.rs         # IDT (exceptions, IRQs, syscall)
│       │       ├── trap.rs        # Trap handlers, PF, GP, syscall dispatch
│       │       ├── paging.rs      # PML4 page table management
│       │       ├── cr3.rs         # CR3 switch helper
│       │       ├── lapic.rs       # Local APIC timer
│       │       ├── ioapic.rs      # I/O APIC IRQ routing
│       │       ├── pci.rs         # PCI enumeration
│       │       ├── smp.rs         # AP boot via INIT/SIPI
│       │       ├── multiboot2.rs  # Multiboot2 info parser
│       │       ├── uart.rs        # COM1 serial (I/O ports)
│       │       ├── virtio_blk.rs  # VirtIO block (PCI)
│       │       ├── virtio_net.rs  # VirtIO net (PCI)
│       │       ├── console.rs     # VGA + serial console
│       │       ├── switch.rs      # Context switch (fxsave/fxrstor)
│       │       ├── user_return.rs # Ring 3→0 syscall path
│       │       ├── cet.rs         # CET disable
│       │       ├── emergency_stack.rs
│       │       ├── platform.rs    # x86_64 platform constants
│       │       └── test.rs        # x86_64 arch tests
│       ├── driver/
│       │   ├── mod.rs             # Driver module root
│       │   ├── uart.rs            # ns16550a UART MMIO (RISC-V)
│       │   ├── virtio.rs          # VirtIO MMIO transport (RISC-V)
│       │   ├── block.rs           # Block I/O dispatch (AHCI→VirtIO→NVMe)
│       │   ├── ahci.rs            # AHCI/SATA driver (x86_64)
│       │   ├── nvme.rs            # NVMe driver (x86_64)
│       │   ├── net.rs             # VirtIO net MMIO (RISC-V)
│       │   ├── fs.rs              # In-memory filesystem (test/syscall)
│       │   ├── ext4.rs            # ext4 module re-export
│       │   ├── ext4_riscv.rs      # ext4 + KarteBlockDevice (RISC-V)
│       │   ├── ext4_x86_64.rs     # ext4 + KarteBlockDevice (x86_64)
│       │   ├── fat32.rs           # FAT32 filesystem
│       │   ├── ramfs.rs           # RamFS (embedded ELFs)
│       │   ├── vfs.rs             # Virtual filesystem layer
│       │   ├── pipe.rs            # Anonymous pipe IPC
│       │   ├── tty.rs             # TTY line discipline
│       │   ├── keyboard.rs        # PS/2 keyboard (x86_64)
│       │   ├── vga.rs             # VGA text mode (x86_64)
│       │   └── p9.rs              # 9P protocol (experimental)
│       ├── mm/
│       │   ├── mod.rs             # Memory management root
│       │   ├── pmm.rs             # Bitmap physical frame allocator
│       │   ├── vmm.rs             # Sv39/x86_64 page tables
│       │   ├── heap.rs            # Kernel heap allocator
│       │   ├── frame.rs           # Physical frame abstraction
│       │   ├── addr.rs            # Virtual address types
│       │   ├── page_table.rs      # Page table entry types
│       │   ├── vma.rs             # Virtual Memory Area tracking
│       │   ├── address_space.rs   # Address space abstraction
│       │   ├── diagnostics.rs     # Page table diagnostics
│       │   └── unsafe_bridge.rs   # Unsafe helpers
│       ├── net/
│       │   ├── mod.rs             # Network module
│       │   ├── iface.rs           # smoltcp Interface + NetStack
│       │   └── device.rs          # Device trait impl (RISC-V)
│       ├── process/
│       │   ├── mod.rs             # Process struct, fork/clone/exec
│       │   └── elf.rs             # ELF loader
│       ├── sync/
│       │   ├── mod.rs             # Sync module
│       │   ├── spinlock.rs        # SpinLock<T> + RAII guard
│       │   ├── int_spinlock.rs    # IntSpinLock (saves/restores SIE)
│       │   └── mutex.rs           # BlockingMutex / YieldMutex
│       ├── sched/
│       │   ├── mod.rs             # Round-Robin scheduler
│       │   └── task.rs            # TaskContext + TaskControlBlock
│       └── syscall/
│           ├── mod.rs             # Syscall dispatch (60+ syscalls)
│           ├── linux.rs           # Linux syscall compatibility layer
│           ├── user_ptr.rs        # User pointer validation
│           └── epoll/
│               ├── mod.rs         # epoll implementation
│               ├── eventfd.rs     # eventfd
│               └── timerfd.rs     # timerfd
├── user/
│   ├── Makefile                   # User program build
│   ├── user.ld                    # RISC-V linker script
│   ├── user-x86_64.ld             # x86_64 linker script
│   ├── shell.rs                   # Interactive shell (v0.5)
│   ├── syscall.rs                 # Shared syscall wrappers
│   ├── hello.S                    # Minimal hello (RISC-V asm)
│   ├── ls.rs, cat.rs, echo.rs     # File utility commands
│   ├── grep.rs, sed.rs            # Text processing
│   ├── wc.rs, head.rs, tail.rs    # Text analysis
│   ├── mkdir.rs, rm.rs            # File management
│   ├── env.rs, pwd.rs             # Environment commands
│   ├── dmesg.rs                   # Kernel log reader
│   ├── tui-demo.rs                # Terminal UI demo
│   ├── test_*.rs                  # User-space tests
│   └── x86_64-bin/                # Pre-built x86_64 binaries
├── tools/
│   └── mkdisk.sh                  # Disk image management
├── vendor/
│   └── ext4_rs/                   # Vendored ext4 library (patched)
└── docs/
    ├── plan-riscv-os.md           # Original development plan
    └── agent/                     # Knowledge base (AI-assisted dev)
```

## Boot Sequence

```
RISC-V:  QEMU → OpenSBI (M-mode) → _start (entry.S) → kmain (main.rs)
x86_64:  QEMU → GRUB (multiboot2) → _start (32-bit) → long mode → _start64 → kmain
                                            │
                            ┌───────────────┤
                            │  1. UART/VGA console init
                            │  2. Multiboot2 memory parsing (x86_64)
                            │  3. Trap vector / IDT setup
                            │  4. Kernel logger init
                            │  5. SMP BSP init
                            │  6. Physical memory (PMM)
                            │  7. Virtual memory (Sv39 / PML4)
                            │  8. Kernel heap
                            │  9. PCI / VirtIO device probe
                            │ 10. Filesystem (ext4 → FAT32 → RamFS)
                            │ 11. VFS + environment
                            │ 12. Linux compat layer
                            │ 13. PLIC / TTY / PS/2 init
                            │ 14. Secondary harts / cores
                            │ 15. Scheduler init
                            │ 16. Load shell from ELF
                            │ 17. Network init (smoltcp)
                            │ 18. Enter user mode
                            └── Enable interrupts → idle loop
```

## Key Technical Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Dual architecture | RISC-V + x86_64 | Broad platform support, real hardware potential |
| Firmware (RISC-V) | OpenSBI | No custom M-mode code needed |
| Firmware (x86_64) | GRUB + Multiboot2 | Standard x86 boot chain |
| Memory model (RISC-V) | Sv39 | Standard RISC-V, well-supported |
| Memory model (x86_64) | PML4 (4-level) | Standard x86_64 long mode |
| Frame allocator | Bitmap | Simple, sufficient |
| Heap allocator | buddy_system_allocator | Mature, `no_std` compatible |
| Filesystem | ext4 (primary), FAT32 (fallback) | ext4 is standard Linux; FAT32 for compatibility |
| Network stack | smoltcp 0.12 | Mature embedded TCP/IP, `no_std` |
| Scheduler | Round-Robin + timer preemption | Simple, deterministic |
| VirtIO transport | MMIO (RISC-V), PCI→MMIO (x86_64) | QEMU virt standard |
| Block devices | AHCI → NVMe → VirtIO | Best performance first |
| Edition | Rust 2024 | Latest safety features |

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `riscv` | 0.16 | RISC-V CSR access, register definitions |
| `riscv-rt` | 0.17 | RISC-V runtime (entry point) |
| `sbi` | 0.3.0 | SBI ecall wrappers (timer, reset, hart_start) |
| `x86_64` | 0.15 | x86_64 structures (GDT, IDT, page tables, instructions) |
| `uart_16550` | 0.3 | x86_64 COM1 serial port driver |
| `raw-cpuid` | 11.0 | x86_64 CPUID instruction wrapper |
| `buddy_system_allocator` | 0.11 | Kernel heap allocator |
| `linked_list_allocator` | 0.10 | Secondary heap allocator |
| `virtio-drivers` | 0.13.0 | VirtIO device drivers (alloc feature) |
| `smoltcp` | 0.12 | TCP/IP network stack |
| `ext4_rs` | vendored | ext4 filesystem (patched: `try_open`, multi-level paths, bug fixes) |
| `starry-fatfs` | 0.4.1 | FAT32 filesystem with LFN support |
| `spin` | 0.12 | Mutex primitives |
| `bitflags` | 2.11 | Page table entry flags |
| `log` | 0.4 | Logging facade (for ext4_rs compatibility) |

## Roadmap

- [x] User-mode process loader (ELF) — RISC-V + x86_64
- [x] ext4 filesystem with multi-level paths
- [x] FAT32 filesystem support (fallback)
- [x] Virtual file system (VFS) layer
- [x] x86_64 architecture support — GRUB boot, 4-level paging, IDT, APIC, PCI
- [x] TCP/IP network stack (smoltcp 0.12)
- [x] Pipe IPC with blocking read/write
- [x] fork() / clone() multi-process with TLS support
- [x] Linux syscall compatibility layer — Go runtime support
- [x] epoll / eventfd / timerfd — Linux-compatible event notification
- [x] Preemptive multi-tasking with Round-Robin scheduler
- [x] SMP multi-core (RISC-V + x86_64)
- [x] TTY subsystem with line editing, raw/cooked mode
- [ ] Copy-on-write (COW) page tables for fork
- [ ] VirtIO GPU / framebuffer console
- [ ] Real hardware support (SiFive, StarFive, PC)
- [ ] User-space threading

## License

MIT

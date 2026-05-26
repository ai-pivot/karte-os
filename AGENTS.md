# KarteOS — AGENTS.md

> A modern RISC-V 64-bit operating system written in Rust 2024 Edition.

## Quick Start

```bash
make build          # Build kernel (cargo build --release)
make run            # Run in QEMU (single core)
make debug          # Run with GDB stub (-S -s)
make test           # Build test kernel + run tests in QEMU
make build-test     # Build test kernel only
make boot-test      # Boot test (normal mode, verifies init sequence)
make clean          # Clean all artifacts
cd user && make     # Build user programs (hello.elf, heap_test.elf)
```

QEMU exit: `Ctrl+A` then `X`.

## ⚠️ Pre-Commit Checklist — MUST follow before every git commit

**CI runs 5 jobs on every push: build, lint (fmt + clippy), test (54 tests), boot-test, smp-test.**
ALL 5 must pass. Before committing, run:

```bash
cd user && make clean && make         # 1. Build user programs (kernel includes hello.elf via include_bytes!)
cargo fmt                             # 2. Format code
cargo build --release -p karte-os-kernel  # 3. Build kernel (must be zero error)
make test                             # 4. Run tests (must be ALL PASSED)
```

**Common CI failure causes:**
- `include_bytes!("../../user/hello.elf")` requires `user/hello.elf` to exist → always build user programs first
- `cargo fmt` differences → always run `cargo fmt` before commit
- Test count changed → update AGENTS.md test count
- Boot-test checks for `"Hello from user"` in QEMU output → verify user program still runs

## Build Requirements

- Rust stable (1.93+), target `riscv64gc-unknown-none-elf`
- `qemu-system-riscv64` (8.2+)
- `gcc-riscv64-linux-gnu` (for user programs, objdump, nm)
- Dependencies: `riscv` 0.16, `sbi` 0.3.0, `buddy_system_allocator`, `virtio-drivers` (alloc feature), `spin`, `bitflags`

## Architecture Overview

S-mode kernel on OpenSBI (M-mode). Identity-mapped Sv39 virtual memory. User programs run in U-mode with dual-path trap handling (trap_entry.S). ELF loader maps user code/data into shared kernel page table (Phase 1 simplification). Round-Robin scheduler with `__switch()` assembly context switch. SMP via SBI `hart_start` for secondary harts.

Boot flow: QEMU → OpenSBI → `_start` (entry.S) → `kmain` → init phases → load user ELF → `sret` to U-mode → user `ecall` → trap handler → syscall dispatch.

## User Programs

- `user/hello.S` — Minimal "Hello from user!" via sys_write + sys_exit
- `user/heap_test.S` — Tests brk heap allocation (single-page + 8-page) with read/write verification
- `user/user.ld` — Linker script: entry at 0x1000
- User programs are embedded into the kernel via `include_bytes!()` at compile time
- **Build before kernel**: `cd user && make` generates `*.elf` files that the kernel references

## Syscall ABI

User programs use `ecall` with `a7=syscall_num`, args in `a0-a5`, return value in `a0`:

| Number | Name | Args |
|--------|------|------|
| 0 | debug_print | (buf, len) |
| 1 | exit | (code) |
| 2 | write | (fd, buf, len) |
| 3 | read | (fd, buf, len) |
| 4 | brk | (addr) — 0 to query, >0 to grow |
| 5 | getpid | () |
| 6 | mmap | (addr, len, flags) |

## GOTCHAS

- **Rust 2024 Edition**: `#[no_mangle]` → `#[unsafe(no_mangle)]`, `extern "C"` → `unsafe extern "C"`, `static mut` → use atomics/Mutex
- **`console_println!` macro** is `#[macro_export]` — call as `crate::console_println!`
- **sbi** 0.3.0 (NOT sbi-rt): `sbi::timer::set_timer()`, `sbi::system_reset::system_reset()`, `sbi::hsm::hart_start()`
- **Direct UART MMIO** for console output — DBCN not available on QEMU SBI 1.0
- **SSTATUS.SUM** must be set in trap_handler for S-mode to access U-mode pages
- **sfence.vma** required after mapping new user pages (TLB stale otherwise)
- **Compressed instructions** — trap skip must check instruction length (16-bit vs 32-bit)
- **sret timing** — disable SIE before sret sequence to prevent timer interrupt preemption
- **QEMU boot hart**: With `-smp N`, OpenSBI may boot on hart 1 (not always hart 0)
- **VirtIO MMIO**: Probe may hang on QEMU virt without explicit devices — disabled for now
- **amoswap/lr/sc**: RISC-V atomic extensions NOT available on bare target
- **sys_write**: Use byte-by-byte `read_volatile` + `console_putchar`, NOT `from_raw_parts` + `from_utf8` (causes bounds panic in S-mode trap context)

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

## Testing

- **54 QEMU integration tests** via `make test` — runs in-kernel test suite in QEMU
- **Test mode**: `cargo build --release --features test_mode` compiles test kernel
- **Test framework**: `kernel/src/test.rs` — TAP-style `run_test(name, || bool)` API
- **Test modules**: Each subsystem has `#[cfg(feature = "test_mode")] pub fn run_tests()`
- **CI**: GitHub Actions runs build + lint + test + boot-test + smp-test on every push
- **Coverage**: PMM (6), VMM (6), Heap (6), FS (15), SpinLock (5), Task (6), Syscall (8) = **52 tests** + 2 mmap tests = **54 tests**

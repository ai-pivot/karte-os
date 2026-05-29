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
cd user && make     # Build user programs (hello.elf, heap_test.elf, file_test.elf, spawn_test.elf)
```

QEMU exit: `Ctrl+A` then `X`.

## ⚠️ Pre-Commit Checklist — MUST follow before every git commit

**CI runs 5 jobs on every push: build, lint (fmt + clippy), test (59 tests), boot-test, smp-test.**
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
- Boot-test checks for `"KarteOS Shell"` in QEMU output (init is the interactive shell) → verify boot reaches user mode

## Build Requirements

- Rust stable (1.93+), target `riscv64gc-unknown-none-elf`
- `qemu-system-riscv64` (8.2+)
- `gcc-riscv64-linux-gnu` (for user programs, objdump, nm)
- Dependencies: `riscv` 0.16, `sbi` 0.3.0, `buddy_system_allocator`, `virtio-drivers` (alloc feature), `spin`, `bitflags`

## Architecture Overview

S-mode kernel on OpenSBI (M-mode). Identity-mapped Sv39 virtual memory. User programs run in U-mode with dual-path trap handling (trap_entry.S). Each process has its own Sv39 page table with kernel mappings copied in. ELF loader maps user code/data into per-process page tables. Round-Robin scheduler with `__switch()` assembly context switch. Multi-process via `sys_spawn` creates independent address spaces. SMP via SBI `hart_start` for secondary harts.

Boot flow: QEMU → OpenSBI → `_start` (entry.S) → `kmain` → init phases → load user ELF → switch satp to user page table → `sret` to U-mode → user `ecall` → trap handler → syscall dispatch. Multi-process: `sys_spawn` creates child process with own page table + kernel stack → registered in scheduler → Round-Robin via timer interrupt → `__switch` context switch → satp restored in trap_handler (per-process address space isolation). New tasks enter via `trap_return_user` assembly label. Last process exit triggers SBI shutdown.

## User Programs

- `user/hello.S` — Minimal "Hello from user!" via sys_write + sys_exit
- `user/heap_test.S` — Tests brk heap allocation (single-page + 8-page) with read/write verification
- `user/file_test.S` — Tests sys_open/close/read/write file operations with data verification
- `user/spawn_test.S` — Tests sys_spawn multi-process: parent spawns child (hello), both run concurrently
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
| 10 | open | (path, path_len, flags) |
| 11 | close | (fd) |
| 30 | spawn | (prog_id, arg) — spawn new process (0=hello, 1=heap_test, 2=file_test, 3=spawn_test) |
| 31 | waitpid | (pid) — wait for child process, returns exit code |

## GOTCHAS

- **Rust 2024 Edition**: `#[no_mangle]` → `#[unsafe(no_mangle)]`, `extern "C"` → `unsafe extern "C"`, `static mut` → use atomics/Mutex
- **`console_println!` macro** is `#[macro_export]` — call as `crate::console_println!`
- **sbi** 0.3.0 (NOT sbi-rt): `sbi::timer::set_timer()`, `sbi::system_reset::system_reset()`, `sbi::hsm::hart_start()`
- **Direct UART MMIO** for console output — DBCN not available on QEMU SBI 1.0
- **SSTATUS.SUM** must be set in trap_handler for S-mode to access U-mode pages
- **sfence.vma** required after mapping new user pages (TLB stale otherwise)
- **Compressed instructions** — trap skip must check instruction length (16-bit vs 32-bit)
- **sret timing** — disable SIE before sret sequence to prevent timer interrupt preemption
- **satp switching**: satp is restored in trap_handler (Rust) after schedule()/__switch(), NOT in trap_entry.S. Conditional write + sfence.vma only when PPN actually changed. New tasks enter via `trap_return_user` label with __switch frame below TrapContext on kernel stack.
- **QEMU boot hart**: With `-smp N`, OpenSBI may boot on hart 1 (not always hart 0)
- **VirtIO MMIO**: Fixed — stride is 0x1000 (page-sized), not 0x200. Requires `-device virtio-blk-device` in QEMU for block device.
- **amoswap/lr/sc**: RISC-V atomic extensions NOT available on bare target
- **sys_write**: Use byte-by-byte `read_volatile` + `console_putchar`, NOT `from_raw_parts` + `from_utf8` (causes bounds panic in S-mode trap context)
- **illegal_instruction handler**: Must NOT use console_println! — the SpinLock in UART output can deadlock when timer interrupts fire during CSR probing. Silently skip_trap_instruction instead.
- **stvec alignment**: `trap_entry` MUST be 4-byte aligned (`.p2align 2` in trap_entry.S). stvec's low 2 bits are the MODE field; if the label lands on a 2-byte boundary the base is truncated and traps vector mid-instruction → infinite illegal-instruction loop.
- **TrapContext = 288 bytes** (36 usizes): x[0..32], sstatus(256), sepc(264), sscratch(272), user_satp(280). `user_satp` is non-zero ONLY for a task's first U-mode entry (via `first_task_shim`, which bypasses trap_handler); trap_return_user switches satp when it's set. Keep trap_entry.S offsets, main.rs, and sched add_user_process in sync (use `size_of::<TrapContext>()`).
- **Scheduler**: `current == MAX_TASKS` is the sentinel meaning "init (shell) is running" (init has no TCB; its sp lives in INIT_TASK_SP). Children occupy real slots 0+. `schedule_exit` returns to init when no Ready child remains.
- **sys_waitpid ABI**: returns exit code (>=0) when child exited, `WAIT_AGAIN` (-1) while still running, `WAIT_ERR` (-2) on error. Exit code 0 must NOT be confused with "still running".
- **shell.elf**: built by `user/Makefile` (via rustc) since the kernel embeds it with `include_bytes!`. `cd user && make` builds it alongside the .S programs.

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

- **59 QEMU integration tests** via `make test` — runs in-kernel test suite in QEMU
- **Test mode**: `cargo build --release --features test_mode` compiles test kernel
- **Test framework**: `kernel/src/test.rs` — TAP-style `run_test(name, || bool)` API
- **Test modules**: Each subsystem has `#[cfg(feature = "test_mode")] pub fn run_tests()`
- **CI**: GitHub Actions runs build + lint + test + boot-test + smp-test on every push
- **Coverage**: PMM (6), VMM (6), Heap (6), FS (15), SpinLock (5), Task (6), Syscall (15) = **59 tests**

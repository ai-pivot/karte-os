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
cd user && make     # Build user programs (hello.elf, heap_test.elf, file_test.elf, spawn_test.elf, shell.elf)
tools/mkdisk.sh init  # Create 64MB FAT32 disk.img
tools/mkdisk.sh put <file>  # Copy host file to disk (accessible in OS)
tools/mkdisk.sh list   # List files on disk
```

QEMU exit: `Ctrl+A` then `X`.

## ⚠️ Pre-Commit Checklist — MUST follow before every git commit

**CI runs 5 jobs on every push: build, lint (fmt + clippy), test (70 tests), boot-test, smp-test.**
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

Synchronization: Three levels of kernel locks. (1) `SpinLock` — for short critical sections (a few instructions, e.g., run queue manipulation). (2) `IntSpinLock` — like SpinLock but also saves/restores `sstatus.SIE` to prevent interrupt-induced deadlocks. (3) `YieldMutex` / `BlockingMutex` — for I/O-bound operations (filesystem, block device); contention yields to the scheduler instead of spinning. **Rule: never hold a SpinLock across block I/O.**

Filesystem: ext4 (preferred, via vendored `ext4_rs` crate) → FAT32 (fallback, via `starry-fatfs`) → RamFS (embedded ELF files). Boot priority: try ext4 mount, on failure try FAT32, finally RamFS-only. ext4 files are pre-loaded on the host via `tools/mkdisk.sh put`; no boot-time injection (too many I/O round-trips).

## User Programs

- `user/hello.S` — Minimal "Hello from user!" via sys_write + sys_exit
- `user/heap_test.S` — Tests brk heap allocation (single-page + 8-page) with read/write verification
- `user/file_test.S` — Tests sys_open/close/read/write file operations with data verification
- `user/spawn_test.S` — Tests sys_spawn multi-process: parent spawns child (hello), both run concurrently
- `user/user.ld` — Linker script: entry at 0x1000
- `user/shell.rs` — Interactive shell (v0.3): launches binaries from PATH, built-ins: cd/exit/export
- `user/syscall.rs` — Shared syscall wrapper module for all Rust binaries
- `user/ls.rs`, `cat.rs`, `echo.rs`, `mkdir.rs`, `rm.rs`, `env.rs`, `pwd.rs` — Independent command binaries
- User programs are embedded into the kernel via `include_bytes!()` at compile time
- **Build before kernel**: `cd user && make` generates `*.elf` files that the kernel references
- **ext4 deployment**: Copy ELF files to ext4 root without `.elf` extension (e.g., `ls.elf` → `ls`)

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
| 32 | exec | (path, path_len) — spawn process from file path (ext4/FAT32/RamFS), searches PATH |
| 40 | ls | (buf, len) — list filesystem contents |
| 41 | mkdir | (path, path_len) — create a directory |
| 42 | unlink | (path, path_len) — delete a file or directory |
| 50 | setenv | (key, key_len, val, val_len) — set environment variable |
| 51 | getenv | (key, key_len, buf, buf_len) — get environment variable, returns value length or -1 |
| 52 | chdir | (path, path_len) — change directory, validates dir exists, updates CWD env var |

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
- **ext4_rs vendored**: patched to add `try_open()` (returns Err on non-ext4 disks instead of panicking) and `Ext4Superblock::is_valid()`. Do NOT upgrade to upstream without these patches.
- **ext4 boot-time injection**: NEVER inject files into ext4 at boot. ext4 metadata ops (inode alloc, bitmap update, dir entry write) require ~30+ block I/O round-trips per file, causing kernel to appear hung. Use `tools/mkdisk.sh put` on the host instead.
- **ext4_rs `write_offset`**: expects exactly BLOCK_SIZE (4096) bytes. Shorter writes need zero-padding to fill the block. The `KarteBlockDevice` adapter handles this.
- **KarteBlockDevice `read_offset`**: MUST return data starting at the exact byte offset, NOT from the containing block's start. ext4_rs's `Block::load(offset)` calls `read_offset(offset)` then `read_as_mut()` from `data[0]` — it expects `data[0]` = byte at `offset`. Returning block-start-aligned data causes inode/dir-entry/block-group-descriptor reads to silently read wrong data.
- **KarteBlockDevice `write_offset`**: ext4_rs calls this with arbitrary (non-block-aligned) offsets and data sizes (e.g., 64-byte BGDT write, 256-byte inode write). Use sector-level read-modify-write to avoid clobbering adjacent data. Do NOT assume block-aligned writes.
- **ext4_rs `balloc_alloc_block`**: defaults to `bgid=1` when `goal=None`. On single-block-group filesystems (e.g., 64MB disk) this skips the only group (bgid=0) and returns ENOSPC immediately. Fixed in vendored copy to start from `bgid=0`.
- **ext4_rs `dir_add_entry`/`try_insert_to_existing_block`**: hardcodes `DirEntryType::EXT4_DE_DIR` for ALL new directory entries (both files and directories). This causes created files to appear as directories in dir listings; `list_root()` filters them out. Fixed in vendored copy to use correct `de_type` based on `child.inode.is_dir()`. Added `Copy+Clone` derive to `DirEntryType`.
- **stvec alignment**: `trap_entry` MUST be 4-byte aligned (`.p2align 2` in trap_entry.S). stvec's low 2 bits are the MODE field; if the label lands on a 2-byte boundary the base is truncated and traps vector mid-instruction → infinite illegal-instruction loop.
- **TrapContext = 288 bytes** (36 usizes): x[0..32], sstatus(256), sepc(264), sscratch(272), user_satp(280). `user_satp` is non-zero ONLY for a task's first U-mode entry (via `first_task_shim`, which bypasses trap_handler); trap_return_user switches satp when it's set. Keep trap_entry.S offsets, main.rs, and sched add_user_process in sync (use `size_of::<TrapContext>()`).
- **Scheduler**: `current == MAX_TASKS` is the sentinel meaning "init (shell) is running" (init has no TCB; its sp lives in INIT_TASK_SP). Children occupy real slots 0+. `schedule_exit` returns to init when no Ready child remains.
- **sys_waitpid ABI**: returns exit code (>=0) when child exited, `WAIT_AGAIN` (-1) while still running, `WAIT_ERR` (-2) on error. Exit code 0 must NOT be confused with "still running".
- **shell.elf**: built by `user/Makefile` (via rustc) since the kernel embeds it with `include_bytes!`. `cd user && make` builds it alongside the .S programs.
- **`user/syscall.rs` `trim()`**: strips trailing \\0 in addition to \\n/\\r/spaces. Why: `get_args()` reads CMD_ARGS into a 512-byte buffer with trailing nulls. Without \\0 trimming, path.len() = 512 which exceeds syscall path_len=256 limit.
- **ext4 `lookup`**: now supports multi-level paths (e.g., "bin/ls") by splitting on '/' and traversing directory tree. Required for PATH-based binary loading from subdirectories.
- **Binary deployment**: ELF files on ext4 disk MUST have `.elf` extension stripped (e.g., `mkdir.elf` → `mkdir`). Shell searches for bare command names via PATH.
- **CWD path resolution**: Kernel `resolve_path()` in `syscall/mod.rs` prepends CWD env var to relative paths for `sys_open`, `sys_mkdir`, `sys_unlink`, `sys_chdir`. `sys_ls` reads CWD directly. CWD is stored as a global env var (`CWD=/test123`), set by `sys_chdir` (called from shell's `builtin_cd`). User programs do NOT need to handle path resolution themselves.
- **ext4 multi-level paths**: `create_directory`, `delete_file`, `write_file` in `ext4.rs` all use `split_last_component()` to support paths like `parent/child`. The parent is resolved via `lookup()`, then the operation targets the last component.
- **`cd` validation**: Shell's `builtin_cd` calls `SYS_CHDIR` which validates the target exists in ext4 via `lookup_path()` + `metadata_of().is_dir()`. Non-existent directories produce "cd: no such directory" error.

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
- **Coverage**: PMM (6), VMM (6), Heap (6), FS (15), SpinLock (5), IntSpinLock (5), Mutex (6), Task (6), Syscall (15) = **70 tests**

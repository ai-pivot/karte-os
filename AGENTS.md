# KarteOS — AGENTS.md

> A modern dual-architecture (RISC-V 64 + x86_64) operating system written in Rust 2024 Edition.

## Quick Start

### RISC-V 64 (primary)
```bash
make                # Build & run on RISC-V (default)
make shell          # Build all + deploy programs + run — ONE COMMAND
make deploy         # Create disk.img + deploy all user programs
make test           # Build test kernel + run tests in QEMU
make clean          # Clean all artifacts

# Disk image management:
tools/mkdisk.sh deploy          # Create disk + install all programs
tools/mkdisk.sh put <file>      # Copy host file to disk
tools/mkdisk.sh get <file>      # Copy file from disk to host
tools/mkdisk.sh list            # List files on disk

# Host shared folder:
make share-riscv HOST_DIR=/tmp/share
```

### x86_64 (secondary)
```bash
make shell-x86      # Build all + deploy programs + run — ONE COMMAND
make iso-x86        # Build ISO + deploy all programs (production build)
make deploy-x86     # Create disk.img + deploy all x86_64 programs
make test-x86       # Run x86_64 tests in QEMU

# Host shared folder:
make share-x86 HOST_DIR=/tmp/share
```

QEMU exit: `Ctrl+A` then `X`.

## ⚠️ Pre-Commit Checklist — MUST follow before every git commit

**CI runs 5 jobs on every push: build, lint (fmt + clippy), test (182 tests), boot-test, smp-test.**
- **First U-mode entry SP convention**: `build_initial_stack` MUST write `user_stack_top` into the TrapContext **x[2] slot (offset 16)**, matching `trap_return_user`, which restores user sp from x[2] and then swaps sscratch↔sp (sscratch ends up holding kernel_stack_top for the next U-mode trap). Writing user_stack_top into the sscratch slot (272) and kernel_stack_top into x[2] makes the first user instruction store to a kernel physical address → immediate store page fault, shell killed at boot (this broke boot-test; fixed 2026-10-08).
ALL 5 must pass. Before committing, run:

```bash
cd user && make clean && make         # 1. Build user programs (kernel includes hello.elf via include_bytes!)
cargo fmt                             # 2. Format code
cargo build --release -p karte-os-kernel  # 3. Build kernel (must be zero error)
make test                             # 4. Run tests (must be ALL PASSED)
```

## Coding Conventions

- **禁止 hack 和 shortcut**：任何时候都不要用临时方案绕过问题。如果脑子里出现"这是一个很大的改动"的想法，立刻实现完整方案。只要有更彻底的方案，就禁止用更小的方案。FakeFile、stub syscall、busy-wait 替代真正 sleep——这些都是禁止的。
- **禁止特殊处理**：永远不要用 `if fd == 0`、`if fd >= 100`、`if slot == X` 这种硬编码的特殊判断来区分不同类型的 fd/task/slot。必须为每种类型建立统一的数据结构和接口（如 `FdInfo` trait 或 enum），所有类型通过同一个接口查询状态。硬编码的 magic number 判断是严格禁止的——发现一个立刻重构为正确方案。
- **DRY (Don't Repeat Yourself)**: Abstract common patterns into helper functions. Example: CR3 switch for page table operations → use a single `with_kernel_cr3(closure)` helper instead of copy-pasting save/switch/restore at every call site. If you find yourself writing the same 5+ lines in multiple places, extract it.
- **Always check full output**: When QEMU test output goes to a file, **read the entire file** (or at least the tail) before forming hypotheses. Never assume what happened based on partial grep results. A single `tail -40` can save 30 minutes of misdiagnosis.
- **Empirical evidence first**: Do not analyze based on assumptions. If a PF loop occurs, add targeted diagnostics (print PTE values, CR3, frame addresses) to verify the hypothesis before attempting fixes. Every fix should be preceded by evidence of the root cause.

**Common CI failure causes:**
- `include_bytes!("../../user/hello.elf")` requires `user/hello.elf` to exist → always build user programs first
- `cargo fmt` differences → always run `cargo fmt` before commit
- Test count changed → update AGENTS.md test count
- Boot-test checks for `"KarteOS Shell"` in QEMU output (init is the interactive shell) → verify boot reaches user mode

## Build Requirements

### RISC-V 64 (primary)
- Rust stable (1.93+), target `riscv64gc-unknown-none-elf`
- `qemu-system-riscv64` (8.2+)
- `gcc-riscv64-linux-gnu` (for user programs, objdump, nm)
- Dependencies: `riscv` 0.16, `sbi` 0.3.0, `buddy_system_allocator`, `virtio-drivers` (alloc feature), `spin`, `bitflags`, `smoltcp` 0.12

### x86_64 (secondary)
- Rust nightly (required for `abi_x86_interrupt` and `#[unsafe(naked)]`), target `x86_64-unknown-none`
- `qemu-system-x86_64` (8.2+)
- `grub-mkrescue` (from `grub-common` / `xorriso`)
- `dosfstools` (mkfs.vfat, for FAT32 disk images) + `e2fsprogs` (mkfs.ext4); `tools/mkdisk.sh` auto-adds `/usr/sbin` to PATH
- Note: `mkdisk.sh deploy*` uses loop mounts (`sudo mount`) — requires a privileged host/container; in restricted environments create the image with `mkdisk.sh init` (tests only need the file to exist)
- Dependencies: `x86_64` crate, `uart_16550`, plus shared deps above

## Architecture Overview

**Dual-architecture**: Architecture-specific code lives under `arch/<arch>/` with `#[cfg(target_arch)]` conditional compilation. `riscv64` is the primary (stable); `x86_64` is secondary (nightly required). Platform constants are in `platform.rs`. RISC-V dependencies (`riscv`, `sbi`, `riscv-rt`) are gated by `[target.'cfg(target_arch = "riscv64")'.dependencies]` in `kernel/Cargo.toml`. x86_64 dependencies (`x86_64`, `uart_16550`) are gated by `[target.'cfg(target_arch = "x86_64")'.dependencies]`.

**x86_64 boot flow**: GRUB ISO → `_start` (32-bit) → disable GRUB paging → set P4/P3/P2 tables → enable PAE → load CR3 → enable long mode → lgdt → enable paging → lretl to `_start64` → `call kmain`. Page tables: P2 with 64×2MB pages (128MB identity map) + P3 with 3×1GB huge pages (1-4GB for MMIO). Build: `cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc`. Run: `grub-mkrescue` → ISO → `qemu-system-x86_64 -cdrom target/karte-os-x86_64.iso -serial stdio`. PCI enumeration via I/O ports (0xCF8/0xCFC). **Block devices**: AHCI (SATA, priority) via PCI class 0x01/0x06/0x01 with BAR5 MMIO, or VirtIO block (fallback) via PCI vendor ID 0x1AF4. **Display**: VGA text mode 80×25 at 0xB8000, dual-output to COM1 serial + VGA. **Input**: PS/2 keyboard (IRQ 1, scancode Set 1) with US layout, feeds into TTY subsystem via `tty::feed_byte()`. Syscall via `int 0x80` with custom naked ISR stub (DPL=3 for Ring 3 access). User programs compiled with `-C relocation-model=static` to avoid PIE/GOT issues. ext4 and FAT32 filesystems available on x86_64 via block I/O dispatch (AHCI first, VirtIO fallback).

S-mode kernel on OpenSBI (M-mode). Identity-mapped Sv39 virtual memory. User programs run in U-mode with dual-path trap handling (trap_entry.S). Each process has its own Sv39 page table with kernel mappings copied in. ELF loader maps user code/data into per-process page tables. Round-Robin scheduler with `__switch()` assembly context switch. Multi-process via `sys_spawn` creates independent address spaces. SMP via SBI `hart_start` for secondary harts.

Boot flow: QEMU → OpenSBI → `_start` (arch/riscv64/entry.S) → `kmain` → init phases → load user ELF → switch satp to user page table → `sret` to U-mode → user `ecall` → trap handler → syscall dispatch. Multi-process: `sys_spawn` creates child process with own page table + kernel stack → registered in scheduler → Round-Robin via timer interrupt → `__switch` context switch → satp restored in trap_handler (per-process address space isolation). New tasks enter via `trap_return_user` assembly label. Last process exit triggers SBI shutdown.

Synchronization: Three levels of kernel locks. (1) `SpinLock` — for short critical sections (a few instructions, e.g., run queue manipulation). (2) `IntSpinLock` — like SpinLock but also saves/restores `sstatus.SIE` to prevent interrupt-induced deadlocks. (3) `YieldMutex` / `BlockingMutex` — for I/O-bound operations (filesystem, block device); contention yields to the scheduler instead of spinning. **Rule: never hold a SpinLock across block I/O.**

Filesystem: ext4 (preferred, via vendored `ext4_rs` crate) → FAT32 (fallback, via `starry-fatfs`) → RamFS (embedded ELF files). Boot priority: try ext4 mount, on failure try FAT32, finally RamFS-only. ext4 files are pre-loaded on the host via `tools/mkdisk.sh put`; no boot-time injection (too many I/O round-trips). **Network**: smoltcp 0.12 TCP/IP stack over VirtIO Net (QEMU user-mode, 10.0.2.15/24). Supports TCP/UDP/ICMP sockets via syscalls 70-77. Timer-driven polling at ~10ms interval. Pipe IPC: anonymous pipes via `sys_pipe` with 4KB ring buffers, supports blocking read/write with scheduler integration. Shell v0.5 supports pipe (`|`), I/O redirection (`>`, `>>`, `<`), command history (↑/↓), and `sys_exec_fd` for passing pipe fds to child processes.

## User Programs

- `user/hello.S` — Minimal "Hello from user!" via sys_write + sys_exit (RISC-V only)
- `user/heap_test.S` — Tests brk heap allocation (RISC-V only)
- `user/file_test.S` — Tests sys_open/close/read/write (RISC-V only)
- `user/spawn_test.S` — Tests sys_spawn multi-process (RISC-V only)
- `user/user.ld` — RISC-V linker script: entry at 0x1000
- `user/user-x86_64.ld` — x86_64 linker script: entry at 0x1000
- `user/shell.rs` — Interactive shell (v0.5): pipe `|`, redirect `>` `>>` `<`, command history ↑/↓, Tab completion, built-ins: cd/exit/export/help/kill
- `user/syscall.rs` — Shared syscall wrapper module for all Rust binaries (cfg-gated per arch)
- `user/ls.rs`, `cat.rs`, `echo.rs`, `mkdir.rs`, `rm.rs`, `env.rs`, `pwd.rs` — Independent command binaries
- `user/grep.rs` — Text search with pattern matching (stdin or file)
- `user/sed.rs` — Stream editor with `s/old/new/g` substitution (stdin or file)
- `user/wc.rs` — Word/line/byte count (stdin or file)
- `user/head.rs` — Output first N lines (stdin or file)
- `user/tail.rs` — Output last N lines (stdin or file)
- `user/dmesg.rs` — Print kernel log buffer (via sys_syslog)
- User programs are embedded into the kernel via `include_bytes!()` at compile time
- **Build before kernel**: `cd user && make` (or `make ARCH=x86_64`) generates `*.elf` files that the kernel references
- **RISC-V assembly programs** (.S files) are cfg-gated and only included on riscv64
- **x86_64 user programs** use `int 0x80` for syscalls (vs RISC-V `ecall`)
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
| 7 | pipe | (fd_ptr) — creates pipe, writes [read_fd, write_fd] to user buf |
| 8 | dup2 | (old_fd, new_fd) — duplicate fd, returns new_fd |
| 10 | open | (path, path_len, flags) — O_CREAT=0x100, O_TRUNC=0x200, O_APPEND=0x400 |
| 11 | close | (fd) |
| 30 | spawn | (prog_id, arg) — spawn new process (0=hello, 1=heap_test, 2=file_test, 3=spawn_test) |
| 31 | waitpid | (pid) — wait for child process, returns exit code |
| 32 | exec | (path, path_len) — spawn process from file path (ext4/FAT32/RamFS), searches PATH |
| 33 | exec_fd | (path, path_len, redir_stdin, redir_stdout) — exec with fd redirection (-1 = keep default) |
| 34 | fork | () — fork current process, returns child_pid (parent) or 0 (child) |
| 40 | ls | (buf, len) — list filesystem contents |
| 41 | mkdir | (path, path_len) — create a directory |
| 42 | unlink | (path, path_len) — delete a file or directory |
| 50 | setenv | (key, key_len, val, val_len) — set environment variable |
| 51 | getenv | (key, key_len, buf, buf_len) — get environment variable, returns value length or -1 |
| 52 | chdir | (path, path_len) — change directory, validates dir exists, updates CWD env var |
| 60 | kill | (pid, sig) — send signal to process (SIGINT=2, SIGKILL=9, SIGTERM=15) |
| 70 | socket | (domain, type, protocol) — domain=2(AF_INET), type=1(TCP)/2(UDP)/3(ICMP) |
| 71 | bind | (fd, addr_ptr, addr_len) — bind socket to sockaddr_in |
| 72 | connect | (fd, addr_ptr, addr_len) — connect TCP to remote |
| 73 | listen | (fd, backlog) — listen on bound TCP socket |
| 74 | accept | (fd) — accept incoming TCP connection |
| 75 | sendto | (fd, buf, len, flags, addr_ptr, addr_len) — send data |
| 76 | recvfrom | (fd, buf, len) — receive data |
| 77 | shutdown | (fd) — close/shutdown socket |
| 80 | ioctl | (fd, cmd, arg) — terminal I/O control (TCSETS, TIOCGWINSZ) |
| 81 | syslog | (buf, len, offset) — read kernel log buffer (for dmesg) |
| 82 | setpriority | (pid, class_code, level) — set scheduling class: 0=RtFifo/1=RtRoundRobin (level 1..=16)/2=Normal/3=AiBatch |
| 83 | getscheduler | (pid) — returns (class_code << 16) \| level, or -ERR_NOENT |

## GOTCHAS

- **Rust 2024 Edition**: `#[no_mangle]` → `#[unsafe(no_mangle)]`, `extern "C"` → `unsafe extern "C"`, `static mut` → use atomics/Mutex
- **`console_println!` macro** is `#[macro_export]` — call as `crate::console_println!`
- **sbi** 0.3.0 (NOT sbi-rt): `sbi::timer::set_timer()`, `sbi::system_reset::system_reset()`, `sbi::hsm::hart_start()`
- **Direct UART MMIO** for console output — DBCN not available on QEMU SBI 1.0
- **Kernel log buffer**: `console_println!` writes to both UART and a 32KB lock-free ring buffer. User-space `dmesg` reads it via `sys_syslog(81)`. Ring buffer is always active, even before filesystem is available.
- **SSTATUS.SUM** must be set in trap_handler for S-mode to access U-mode pages
- **sfence.vma** required after mapping new user pages (TLB stale otherwise)
- **Compressed instructions** — trap skip must check instruction length (16-bit vs 32-bit)
- **sret timing** — disable SIE before sret sequence to prevent timer interrupt preemption
- **satp switching**: satp is restored in trap_handler (Rust) after schedule()/__switch(), NOT in trap_entry.S. Conditional write + sfence.vma only when PPN actually changed. New tasks enter via `trap_return_user` label with __switch frame below TrapContext on kernel stack.
- **QEMU boot hart**: With `-smp N`, OpenSBI may boot on hart 1 (not always hart 0)
- **VirtIO MMIO**: Fixed — stride is 0x1000 (page-sized), not 0x200. Requires `-device virtio-blk-device` in QEMU for block device.
- **amoswap/lr/sc**: RISC-V atomic extensions NOT available on bare target
- **sys_write**: Use byte-by-byte `read_volatile` + `console_putchar`, NOT `from_raw_parts` + `from_utf8` (causes bounds panic in S-mode trap context)
- **VirtIO Net MMIO version**: QEMU virt reports MMIO version **1**, NOT version 2. Do NOT filter by `version != 2` or the net device will never be found. The version field at offset 0x04 reads as 1 for all QEMU virt VirtIO devices.
- **VirtIO Net slot**: On QEMU virt with `-device virtio-blk-device` + `-device virtio-net-device`, net is at slot 6 (0x10007000), block is at slot 7 (0x10008000). Slots 0-5 are empty. Probe must scan all 8 slots.
- **QueueMem repr(C)**: `QueueMem` struct in `driver/net.rs` MUST use `#[repr(C)]`. Without it, Rust compiler reorders fields, causing VirtIO DMA to write to wrong addresses → memory corruption → VMM panic during user program loading.
- **Network init timing**: Network initialization (`init_net_device()` + `NetStack::init()`) must happen AFTER user program loading completes. The DMA buffer setup (~25KB) can interfere with VMM page table allocation if done before user space is established. Network init is placed after `process::add_process()` in `kmain()`.
- **smoltcp Device trait**: `receive()` must drop the NET_STATE lock before returning tokens (tokens re-acquire the lock in `consume()`). Holding the lock across token return causes deadlock.
- **smoltcp TCP connect**: Requires `Interface::context()` call for source address selection: `sock.connect(cx, remote_endpoint, local_port)`. Omitting `cx` causes compile error.
- **Network poll in timer ISR**: `NetStack::poll()` is called from the timer interrupt handler. It acquires `NET_STACK` mutex. If any syscall also holds this mutex during interrupt, deadlock occurs. Current design is safe because ISR disables interrupts before acquiring spin::Mutex.
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
- **Scheduler**: Shell/init is a normal `TaskKind::User` task. The scheduler has a typed `TaskKind::Idle` fallback for the no-ready-task case; do NOT reintroduce `current == MAX_TASKS`, `INIT_TASK_SP`, or slot-number checks to identify init. `schedule_exit` switches to the next ready user task or idle.
- **sys_waitpid ABI**: returns exit code (>=0) when child exited, `WAIT_AGAIN` (-1) while still running, `WAIT_ERR` (-2) on error. Exit code 0 must NOT be confused with "still running".
- **shell.elf**: built by `user/Makefile` (via rustc) since the kernel embeds it with `include_bytes!`. `cd user && make` builds it alongside the .S programs.
- **`user/syscall.rs` `trim()`**: strips trailing \\0 in addition to \\n/\\r/spaces. Why: `get_args()` reads CMD_ARGS into a 512-byte buffer with trailing nulls. Without \\0 trimming, path.len() = 512 which exceeds syscall path_len=256 limit.
- **ext4 `lookup`**: now supports multi-level paths (e.g., "bin/ls") by splitting on '/' and traversing directory tree. Required for PATH-based binary loading from subdirectories.
- **Binary deployment**: ELF files on ext4 disk MUST have `.elf` extension stripped (e.g., `mkdir.elf` → `mkdir`). Shell searches for bare command names via PATH.
- **CWD path resolution**: Kernel `resolve_path()` in `syscall/mod.rs` prepends CWD env var to relative paths for `sys_open`, `sys_mkdir`, `sys_unlink`, `sys_chdir`. `sys_ls` reads CWD directly. CWD is stored as a global env var (`CWD=/test123`), set by `sys_chdir` (called from shell's `builtin_cd`). User programs do NOT need to handle path resolution themselves.
- **ext4 multi-level paths**: `create_directory`, `delete_file`, `write_file` in `ext4.rs` all use `split_last_component()` to support paths like `parent/child`. The parent is resolved via `lookup()`, then the operation targets the last component.
- **ext4 module structure**: `ext4.rs` re-exports architecture-specific impl via `#[path]` + `pub use ext4_arch::*`. Do NOT add a separate `pub mod ext4_x86_64` or `pub mod ext4_riscv` to `driver/mod.rs` — doing so compiles the same file twice, creating duplicate `static` variables (EXT4_AVAILABLE, EXT4_FS) where only one instance is initialized. Always use `crate::driver::ext4::*` for ext4 operations.
- **`cd` validation**: Shell's `builtin_cd` calls `SYS_CHDIR` which validates the target exists in ext4 via `lookup_path()` + `metadata_of().is_dir()`. Non-existent directories produce "cd: no such directory" error.
- **Pipe fd lifecycle**: `sys_pipe` allocates a pipe with refcount=2 (read+write end). Each `sys_close` on a pipe fd decrements refcount and calls `pipe_close_read()`/`pipe_close_write()`. When both ends are closed, the pipe is freed. Pipe fds are inherited by child processes via `sys_exec_fd` (increments refcount). Shell must close its pipe fds after launching children.
- **Pipe blocking**: `pipe_read` blocks when buffer is empty and write end is open (calls `schedule_block`). `pipe_write` blocks when buffer is full. Blocked tasks are woken by the opposite end's close/write operation. **Init (shell) must never block on a pipe** — schedule_block on init returns immediately.
- **FdType routing**: `sys_read`/`sys_write` check `FdType::PipeRead`/`PipeWrite` before falling through to Stdio/TTY/UART. fd=0/1/2 are pre-allocated as `FdType::Stdio` but can be overridden via `sys_dup2` or inherited from parent via `sys_exec_fd`.
- **O_APPEND**: New flag `O_APPEND=0x400` for `sys_open`. Shell's `>>` redirect uses O_CREAT|O_APPEND. Not yet implemented at kernel write level (appends via write_at_end pattern).
- **sys_fork**: Deep-copies user page table (no COW). Copies fd table including pipe refs (increments refcount). Child resumes at same sepc — currently always returns parent's PID (child path needs trap_context manipulation to return 0).
- **x86_64 PTE flags**: x86_64 PTE bit layout is completely different from RISC-V. Present(0), R/W(1), U/S(2), PWT(3), PCD(4), A(5), D(6), PS(7), G(8), NX(63). `PTEFlags` is cfg-gated per architecture. Non-leaf PTEs MUST have User bit set for Ring 3 page walks to work.
- **x86_64 IDT syscall DPL**: `int 0x80` for syscalls MUST have DPL=3 in the IDT entry, otherwise Ring 3 code triggers GP Fault. Default `set_handler_fn` sets DPL=0; must patch attribute byte (set bits 5-6).
- **x86_64 user programs**: Must compile with `-C relocation-model=static` to avoid PIE. PIE generates GOT-based indirect calls that jump to address 0 without a dynamic linker.
- **x86_64 copy_kernel_mappings**: Must NOT identity-map the user address range (0..1MB) into user page tables, or ELF loader's `translate_user` check will find stale mappings and skip frame allocation, writing shell code to wrong physical pages.
- **x86_64 UART**: COM1 uses I/O ports (`in`/`out`), NOT MMIO. `tty.rs` uses `arch::uart` (port I/O) on x86_64, not `driver::uart` (MMIO). RISC-V UART at 0x10000000 is MMIO.
- **x86_64 no CR3 switch**: ~~Currently user code runs with kernel CR3~~ **已实现 CR3 页表隔离！** 每个进程有独立的用户页表，`trap_return_user` 在 iretq 前切换 CR3，`timer_trap_handler` 在 schedule() 返回后恢复当前进程的 CR3。`copy_kernel_mappings` 映射了内核代码/VGA/LAPIC/IOAPIC/PCI MMIO 到每个用户页表。
- **x86_64 preemptive scheduling**: Timer ISR uses custom naked function (NOT `extern "x86-interrupt"`) that saves complete 15-GP-register TrapContext. Calls `schedule()` → `__switch()` for Round-Robin preemption every ~10ms. IDT entry is manually constructed (same as syscall stub) because naked functions can't use `set_handler_fn`.
- **x86_64 FPU/SSE save**: `__switch` in `switch.S` uses `fxsave64`/`fxrstor64` (512 bytes) to save/restore FPU/SSE state on context switch. Stack frame size = 6 callee-saved + 1 ret addr + 512 fxsave = 568 bytes. New task initial stack must include zeroed fxsave area.
- **x86_64 TSS.RSP0 update**: `schedule()` and `schedule_exit()` both call `gdt::set_kernel_rsp0()` after `__switch` to update the kernel stack pointer for Ring 3→Ring 0 interrupt transitions.
- **x86_64 page fault lazy allocation**: Page fault handler uses VMA (Virtual Memory Area) tracking. Lazy allocation applies to both user-mode faults AND kernel-mode faults with user CR3 (e.g., sys_write reading lazy mmap'd user buffer). The `can_lazy_alloc` flag covers both cases. VMA table records start/end/prot for each mmap region; PF handler validates VMA before allocating. PROT_NONE VMAs refuse allocation → segfault.
- **x86_64 GP fault handling**: GP fault handler now terminates user processes instead of `loop {}` deadlock. Kernel-mode GP faults still halt.
- **x86_64 VGA text mode**: `driver/vga.rs` writes directly to 0xB8000 (identity-mapped). `console_putchar` in `platform.rs` outputs to both COM1 and VGA simultaneously. `tty::echo()` also outputs to both. VGA driver uses `AtomicBool` for initialization state; writing before `init()` is a no-op.
- **x86_64 PS/2 keyboard**: `driver/keyboard.rs` handles Set 1 scancodes from I/O port 0x60 (IRQ 1). `keyboard_handler` in `idt.rs` calls `keyboard::handle_scancode()` which translates to ASCII and feeds `tty::feed_byte()`. Extended keys (E0 prefix) handled. Shift/Caps Lock state tracked via atomics.
- **x86_64 AHCI/SATA**: `driver/ahci.rs` implements AHCI DMA via BAR5 MMIO. PCI discovery in `pci.rs::find_ahci()` (class 0x01/0x06/0x01). Port memory uses `SafePortMem` wrapper with `UnsafeCell` + manual `Sync` impl (Rust 2024 safe). Block I/O dispatch in ext4/fat32: tries AHCI first, falls back to VirtIO. DMA memory allocated via `pmm::alloc_frame()`.
- **x86_64 ext4/fat32**: Full implementations (not stubs!) available via block I/O dispatch. ext4 uses `KarteBlockDevice` adapter (same as RISC-V but calling x86_64 block I/O). FAT32 uses `Fat32Storage` with block I/O dispatch. Both support AHCI and VirtIO block devices.
- **x86_64 TLB flush**: `sys_brk` and `sys_mmap` now call `flush_tlb()` (x86_64: `x86_64::instructions::tlb::flush_all()`) after mapping new pages. Previously missing — could cause stale TLB entries.
- **x86_64 sys_read schedule**: stdin read loop now calls `schedule()` instead of `pause` spin-loop. Yields CPU to other tasks while waiting for keyboard input.
- **x86_64 SMP**: `arch/x86_64/smp.rs` implements multi-core boot via LAPIC INIT/SIPI IPIs. AP trampoline (`ap_trampoline.S`) at 0x7000 transitions real→protected→long mode. Per-CPU GDT+TSS (indexed by CPU ID, MAX_CPUS=4). Shared IDT (build once, load per-CPU). `start_secondary_harts(total)` starts APs; each runs `secondary_cpu_entry()` → GDT init → LAPIC init → timer → schedule loop.
- **x86_64 syscall ISR `sti`**: `syscall_isr_stub` (int 0x80) MUST NOT use `sti` between push registers and `call syscall_handler_impl`. The Timer ISR uses IST (shared across all syscalls on the same CPU). If `sti` enables interrupts and the Timer ISR fires before `call`, the Timer ISR's IST pushes overwrite the syscall's saved registers on the IST stack, corrupting syscall arguments. Instead, keep interrupts disabled throughout the syscall handler; `iretq` restores IF from user-mode RFLAGS (IF=1) when returning to Ring 3.
- **x86_64 UART input via `-serial stdio`**: QEMU's `-serial stdio` mode does NOT reliably deliver stdin data to the UART RX FIFO via polling (`getchar()`). Use Unix socket chardev (`-chardev socket,id=serial0,path=/tmp/serial.sock,server=on,wait=off -serial chardev:serial0`) for reliable UART input. The COM1 ISR (IRQ4 via IOAPIC) works correctly with the socket chardev but may not trigger with `-serial stdio`.
- **x86_64 boot identity mapping range**: boot.S P2 table MUST cover ALL physical memory (256×2MB = 512MB for QEMU `-m 512M`). Originally only 64 entries (128MB) caused `map()` to access unmapped PT frames via identity mapping → GP fault or corrupt data.
- **x86_64 CR3 switching for page table ops**: User page table's identity mapping entries get overwritten during ELF loading. When `map()` accesses PT frames via identity mapping under user CR3, it may read ELF data instead of PT structures. **Fix**: Switch to kernel CR3 (clean identity mapping) before any `map()`/`translate_user()`/`unmap_user()` call on user page tables, switch back after. This applies to PF handler lazy allocation AND syscall paths (mmap/mprotect/brk). Use the `with_kernel_cr3()` helper — do NOT inline CR3 switch logic at every call site.
- **x86_64 `current_page_table_root()` vs `current_page_table_ppn()`**: Two different mechanisms to get the page table root. `current_page_table_root()` uses lock-free `AtomicUsize` (safe from trap handlers). `current_page_table_ppn()` uses `PROCESS_TABLE.lock()` (NOT safe from trap handlers, may deadlock). **Always use `current_page_table_root()`** in trap handlers and PF handler. The locked version is only for non-interrupt contexts.
- **x86_64 ELF identity map corruption**: When `translate_user()` returns a frame where `frame == vaddr` (identity mapping from `copy_kernel_mappings`), `map()` or `write_bytes` will corrupt that physical frame. Always check and skip identity-mapped frames, allocating new ones instead. This prevents ELF data from overwriting critical structures (CR3, PT tables, kernel stack) that share physical addresses within the ELF vaddr range.
- **x86_64 Go binary support**: Go runtime (e.g., a static Go CLI binary ~69MB, NOT committed to the repo — build or obtain it separately, place at repo root or pass to `tools/mkdisk.sh put`) requires: `mmap` (PROT_NONE reservation + PROT_RW allocation), `mprotect`, `brk`, `openat` (for `/proc/version`, `/etc/...`), `write`. Go panics with "failed to determine kernel version" if `openat` for version info returns ENOENT — need to implement `uname` syscall or fake `/proc/version` to proceed further.
- **Arc<spin::Mutex<FdTable>>**: `Process.fd_table` is `Arc<spin::Mutex<FdTable>>`, not `Option<FdTable>`. CLONE_FILES threads share the same fd table via `Arc::clone()` (POSIX semantics). `fork` deep-copies into a new `Arc`. `with_fd_table` clones the Arc under PROCESS_TABLE lock, then locks the fd_table — NEVER hold PROCESS_TABLE lock while accessing fd_table contents (deadlock risk with nested locks).
- **EPOLLET edge-triggered**: `EpollEntry` tracks `last_revents` per fd. For `EPOLLET` entries, `epoll_wait` only reports when `revents` changes from `last_revents`. Without this, every `epoll_wait` returns EPOLLOUT for writable fds → Go netpoller spins forever. `epoll_ctl MOD` resets `last_revents` to 0.
- **EPOLLERR/EPOLLHUP**: Do NOT unconditionally return EPOLLERR/EPOLLHUP when the caller requests them. Only report on real errors or when fd is closed/invalid. Unconditional reporting causes Go's netpoller to loop infinitely.
- **x86_64 xbot testing**: a static Go CLI binary (e.g., xbot-cli) creates `.xbot/` directory and files on first run. No pre-existing files needed on disk. The binary itself is not committed (69MB); `scripts/boot-x86_64.sh` falls back to shell when absent.
- **x86_64 copy_kernel_mappings huge pages**: Only copy PDP entries with PS=0 (PD table pointers). Skip 1GB huge pages (PS=1) which map MMIO (LAPIC 0xFEE00000, IOAPIC 0xFEC00000). These must NOT be user-accessible — kernel accesses MMIO via `with_kernel_cr3()`.
- **x86_64 user page table MMIO**: LAPIC, IOAPIC, and PCI MMIO are NOT mapped into user page tables. Any user-space access to these addresses triggers PF (by design). Go runtime should never access MMIO directly.
- **Linux syscall compatibility layer**: `linux.rs` translates Linux syscall numbers to KarteOS native syscalls. Key implemented: `uname(122)` (fake "Linux 6.1.0"), `getcwd(79)`, `sysinfo(99)`, `gettimeofday(96)`, `sched_getaffinity(204)`, `clock_gettime(228)`, `mprotect(10)`, `fstat(5)` (via `linux_fstat` in mod.rs), `lseek(8)`, `wait4(61→84)` (POSIX status encoding, returns child pid), `execve(59→85)` (NUL path + argv/envp; powers busybox). Go runtime depends on these for initialization. x86_64 syscall numbers must match standard Linux x86_64 ABI (NOT RISC-V numbers).
- **RISC-V user-memory access after schedule()**: satp is NOT restored on kernel-mode resumption (it keeps pointing at whichever task ran last; the user page table is only installed at trap_return_user). Kernel paths that touch user memory AFTER a schedule() switch inside a syscall (waitpid polling, blocking reads/writes) MUST NOT dereference the user VA directly — it would land in whichever process ran last. Use `user_translate()` (walks the syscall owner's page table → physical address) via `user_read_u8`/`user_write_bytes`. x86_64's user_read_u8 explicitly loads the owner CR3 for each access and is safe, but `current_page_table_root()` is also switched by `set_current_process_for_slot` — new user-memory helpers must always go through `user_translate`.
- **Pipe refcounts are PER-END**: `PIPE_REFS[i] = [read_fds, write_fds]` under a single SpinLock. An end is marked closed (EPIPE for writers / EOF for readers) only when its LAST fd closes — a shell closing its pipe fds right after spawning children must NOT tear down the channel. Never reintroduce a single combined refcount, and never mark an end closed while other fds still reference it (the old `with_pipe(close_read)` + `dec_ref` pairing caused exactly that: `echo x | cat` lost all data). Lock order: PIPE_TABLE→PIPE_REFS in alloc_pipe; PIPE_REFS alone (released) → PIPE_TABLE in close paths — no reverse nesting.
- **Linux dup2(33) is handled ONLY on the `syscall`-instruction path** (`dispatch_linux_syscall` line `33 => linux_dup2`), because native int-0x80 number 33 is SYS_EXEC_FD and KarteOS shell pipelines depend on it. Do NOT add `id == 33` to the translate() interception layer — that hijacks exec_fd and breaks every pipeline ("command not found: <cmd>" on both stages).
- **Linux fcntl has ONE implementation** (`linux_fcntl` in syscall/mod.rs, reachable via the `syscall`-instruction dispatch table entry `72`). It covers F_DUPFD(0)/F_DUPFD_CLOEXEC(1030) (per-end pipe ref inc), F_GETFD/F_SETFD, F_GETFL (returns desc.flags), F_SETFL, and POSIX flock commands. Do NOT add a separate translate() interception for 72 — int-0x80 native 72 is SENDTO; busybox never issues fcntl through int 0x80. Duplicate handlers drifted before and cost hours of debugging.
- **sys_ioctl accepts fd 0/1/2**: busybox ash probes its controlling terminal through stderr (fd 2) with TCGETS; restricting the ioctl tty family to fd 0/1 makes ash report "can't access tty; job control turned off" and misbehave. TCGETS/TCSETS use real Linux termios layout (ICANON=0x2, ECHO=0x8 in c_lflag) and TIOCGWINSZ writes a proper 4×u16 winsize struct.
- **Kernel heap diagnostics**: `IrqSafeHeap::alloc` prints `[alloc] FAIL size=… align=…` unconditionally on allocation failure (compliant with the runtime-logging policy: condition is the failure itself, not a counter). There is a timing-sensitive LayoutError race in the kernel heap around heavy exec paths (busybox ash/echo sometimes panic in linked_list_allocator hole.rs:422); adding ANY serial print (e.g. per-syscall debug lines) makes it disappear — treat heap-corruption suspects with a serial-timing sensitivity when hunting it.
- **ELF orphan sections in user linker script (RISC-V)**: rustc places `static mut` arrays into module-path-named sections (`.bss.llm.KV` etc.); a `*(.bss)` glob in user.ld does NOT match them, so ld emits them as orphan sections that can land *before* the RW LOAD segment's vaddr (observed: section at 0x31a008 while the PHDR vaddr started at 0x31b000) — those pages are never mapped and the program faults on first access to the static. user.ld now globs `*(.data .data.*) *(.sdata .sdata.*) *(.bss .bss.*) *(.sbss .sbss.*)`. Any future user program with `static` state depends on this.
- **llm KV-cache inference port (WIP, parked)**: rewriting llm.rs from full-block forward to single-token KV-cache decoding (~9x less MACs/token) hit an unresolved fault family — stores/loads whose fault address is always `target ± 0x80000000` (0x8035a1e8 = F1V+2GB, 0xffffe220 = stack−2GB−x) with identical signatures under opt-level 1 and 3, and drifting addresses between rebuilds. Normal full-block forward with the same statics works, sp inside the faulting task is healthy (0x7ffff0c0). Parked via git history; revisit before M2 (needed for on-device token rate ≥2 tok/s).
- **ELF shared-page permissions (RISC-V)**: two PT_LOAD segments may share one 4K page (e.g. text's last page with .bss). The loader keeps one frame and merges permissions — and on RISC-V the merge MUST be a UNION of the existing and new PTE permission bits (R/W/X are permissive). `merge_page_flags` used to return only the new flags on riscv64, silently stripping X from a text page shared with .bss → instruction page fault at first call into that page. Fixed via `vmm::walk_mapping()` reading the live PTE. user/user.ld now also ALIGNs data/bss to 4K so segments stay page-disjoint in the common case; the union fix remains the source of truth for any future shared page.
- **Vector extension context (M0-V)**: TrapContext is 832 bytes on RISC-V (288 GP/CSR + 512 v[0..31] + vtype/vl/vstart/vxsat); U-mode traps save/restore the full vector area, S-mode traps use only the first 288 bytes (S-mode never touches vector state). ALL vector code is gated on `HAS_VECTOR_EXT` (probed once at boot) — **misa is an M-mode CSR, so the probe MUST run only after stvec is armed** (the illegal-instruction trap must land on a working handler; probing first = trap loop = boot-spam restart, cost hours). **Also: do NOT probe via misa::read() from S-mode** — the read is silently skipped and yields garbage even on V-capable harts (QEMU 6.2 executes vector instructions fine while misa reports no 'V'), which silently disabled kernel-side V save/restore → cross-task vector register clobbering. Probe by *executing* `vsetvli` with a sentinel in t0 (t0 becomes vl on V harts, keeps the sentinel when skipped) — see detect_vector_ext. In assembly, wrap vector instruction blocks in `.option push` / `.option arch, +v` / `.option pop` (the riscv64gc target has no V feature; LLVM's `.option arch` rejects version numbers like `+v1p0` — use plain `+v`), and remember RVV loads/stores take `(reg)` operands only — no immediate offsets. Cross-arch sync: keep trap_entry.S offsets, arch/riscv64/trap.rs and sched::add_user_process in sync via size_of::<TrapContext>().
- **Shell quoting is quote-aware** (user/shell.rs): `split_pipe()` does not treat `|` inside single/double quotes as a pipe separator, and `launch()`'s argv splitter treats a quoted segment as ONE argument with the quotes stripped. This is what lets `busybox ash -c "echo hi | cat; echo PIPE_IN_C_OK"` reach ash intact — the naive splitter used to (a) cut the quoted command at the inner `|` and (b) strip only the OPENING quote, handing ash `hi"` → "unterminated quoted string".
- **Known issue — ash forked pipeline children GP-fault**: `busybox ash -c "echo hi | cat; ..."` parses and runs the pipeline (later plain commands output fine), but ash's fork+exec'd busybox children terminate with code 99 (x86_64 GP-fault handler) losing the in-pipe data. Native pipelines (`echo hi | cat` via exec_fd) work end-to-end. Suspects: fork's 2MB busybox page-table deep-copy or ash's own stack juggling. Next iteration.
- **ext4 large-file read investigation (RISC-V, RESOLVED-BENIGN)**: an early M1 observation claimed user-space reads of the 3.24 MB weights.bin stall after ~4 KB (`cat` stopped emitting). Systematic investigation with the diagnostic kernel found **no fault in either layer**: (1) single-shot `ext4::read_file` of the full 3.24 MB passes (`ext4_large_file_read`), and (2) vfs-layer chunked reads via a live fd (24×512 B) also pass (`ext4_large_file_chunked_read`) — both added to the test_mode FS suite (117/117). The original stall could not be reproduced under a diagnostic environment and is attributed to the pre-diagnostic observation chain (stdin delivery flakiness + QEMU timeout truncation). M1 still ships weights embedded in the ELF (faster boot, no runtime file dependency); the ext4 read path is proven correct by the new tests.
- **Known issue — VFS read after openat stalls (x86_64)**: busybox `cat /file` opens fine via openat (fd allocated) but the subsequent read never completes (no output, no exit); native `cat /file` on the same file works. Something between the syscall-instruction read path and the VFS/ext4 read stalls. Next iteration.
- **Testing-methodology trap — test_mode kernel has no shell**: the test_mode build runs the TAP suite and never enters the interactive shell; booting it and expecting the shell banner "stalls" after `[sched] Switching to next task: 1` by design. This was misread as a RISC-V full-kernel shell-boot regression during M0-V debugging (burned a bisect cycle); the release kernel boots to the shell fine on every commit tested (af2f7a7 through HEAD). Always build WITHOUT --features test_mode when exercising user programs/shell on RISC-V.
- **x86_64 `int 0x80` vs `syscall` instruction**: Two separate dispatch paths. `int 0x80` → KarteOS native syscalls (mod.rs `dispatch_inner`). `syscall` instruction (MSR LSTAR) → Linux compat layer (mod.rs `dispatch_linux_syscall`). Go uses `syscall` instruction exclusively. The two paths have different syscall number spaces — no conflict between KarteOS number 5 (SYS_GETPID via int 0x80) and Linux number 5 (fstat via syscall).
- **x86_64 SYSCALL uses TSS.RSP0**: `syscall_entry` reads per-task kernel stack from TSS.RSP0 (via `TSS_RSP0_ADDR` pointer), NOT from a global `SYSCALL_KSP`. This is critical because Timer ISR can preempt during CPL=0 SYSCALL execution (IST=0 → no stack stack). Per-task stacks prevent corruption when `__switch()` saves/restores SPs. `TSS_RSP0_ADDR` is a `#[unsafe(no_mangle)]` global pointing to the TSS RSP0 field.
- **x86_64 SYSCALL rbx is per-frame, NOT global**: Original user RBX is pushed onto the syscall's own kernel stack frame (not a global variable). If a syscall triggers `schedule()` and another task's syscall runs, the global would be overwritten. The return path restores RBX from `[rsp - 88]` (relative to the iretq frame), ensuring each syscall restores its own saved value.
- **x86_64 ISR stubs MUST be `#[unsafe(naked)]`**: Keyboard (IRQ1) and COM1 UART (IRQ4) ISR stubs use `naked_asm!` and save/restore ALL 15 GP registers. Non-naked functions get a compiler prologue that only saves rax, and the old code only saved callee-saved registers. This clobbers caller-saved registers (especially rax = syscall return value) when the ISR fires between a SYSCALL return and userspace reading RAX. All ISRs that can interrupt user code must preserve the full register state.
- **x86_64 ext4 sector write-through cache**: `KarteBlockDevice` in `ext4_x86_64.rs` maintains a sector-level write-through cache (`SECTOR_CACHE`, 2048 entries). ext4_rs has no in-memory block cache; without this cache, `write_offset`'s read-modify-write at sector granularity causes bitmap/inode/bgdt updates to clobber each other when sharing the same physical sector. The cache ensures write-after-write consistency: `write_offset` writes to disk AND cache, `read_offset` checks cache first.
- **mmap lazy allocation + VMA tracking**: All MAP_ANONYMOUS mmap creates VMA entries (start/end/prot) but does NOT allocate physical frames. The PF handler lazily allocates zeroed frames on first access, validated against the VMA table. PROT_NONE mappings refuse PF allocation. This is the standard Linux behavior; Go relies on it for `sysReserve`→`sysMap`→`sysUsed`→`sysUnused` lifecycle.
- **madvise MADV_DONTNEED decommits**: MADV_DONTNEED/MADV_FREE releases physical frames (removes PTEs, frees frames via `unmap_user`). MADV_POPULATE/WILLNEED pre-allocates frames. Go's `sysUnused` calls MADV_DONTNEED to release memory; `sysMap` re-commits via mmap(MAP_FIXED). The VMA entry persists across commit/decommit cycles.
- **QEMU 6.2 socket-netdev 跨实例帧互通实测未达**（P2.5 双机演示）：`-netdev socket,udp=` 模式实为 multicast 专用（单播隧道对端收不到）；TCP 隧道模式（A listen/B connect）A/B rx 均为 0，而内核侧 send 无错误、bind/poll 正常、159 单测全绿——问题在 QEMU 网络层语义，需 host bridge/TAP 或 QEMU 7+ 复测。demo 脚本保留双模式配置（scripts/demo-dual.sh）。多任务并发时 QEMU stdio 观察层字节交错同理（内核 trace 证明 syscall 全到达）——**调试此类问题先做内核侧 trace 分界**（如 sys_write 入口打印），不要在观察层盲调
- **smoltcp 0.12 `connect` rejects `local_port=0`**（P3.3 MQTT 根因）：返回 `Unaddressable` 而非自动分配本地端口（旧版语义已变）。内核 `NetStack::connect` 必须自行分配临时端口（49152..65534 ephemeral range），否则所有出站 TCP 永久失败。调试外发 TCP 时先确认这个分支：`[net] TCP connect err: Unaddressable` 出现 = local_port==0 或远端地址非法
- **P3.3 MQTT 遗留 — SYN→SYN-ACK 最后一环**（2026-10-08，VirtIO legacy 化已修复大半）：**根因修复三件套已落地并验证**：① smoltcp 0.12 拒绝 local_port=0（已修，临时端口 49152+）；② **QEMU virtio-mmio version=1 是 legacy 接口**——QueuePFN(0x040)=页帧号+QueueAlign(0x03C)，不能用 modern 的 64 位 desc/avail/used 地址寄存器（0x080 起）；③ **legacy vring 布局必须匹配 QEMU 的 QUEUE_ALIGN=4096 计算**（desc@0、avail@128、used@4096——QueueMem 用 #[repr(align(4096))] + avail_buf 撑到 4096 边界、used_buf 4096）；④ rx 队列必须预填充 WRITE desc 到 avail ring + recv_packet 用单调 used cursor（RX_LAST_USED）+ desc 归还。**验证证据**：完整重部署后（mq31）rx 收 arp=71+ipv4=4414 帧、tx 发出 66B SYN、167/167 全绿；TCP fd=1 终态 Closed（connect 重试耗尽）。**下一诊断轴 — QEMU 运行非确定性**：同内核同盘多次运行行为不同（mq31 全通 rx 洪水 vs mq24/25/30/35 tx=1/census=0 仅 126 行输出）——announce/drt_net_tick 在部分运行中未启动；建议 QEMU `-icount` 固定虚拟时序或多次采样定位。探针保留：[tx] send_packet/notify used_idx、RX_TYPE_*/RX_V4_* census 5s 打印、TCP fd state dump
- **虚拟 ESP32 真测进展**（scratch32，QEMU riscv32 virt M-mode）：① global_asm! 裸入口修复（新 _start 有栈变量→编译器序言在 sp=0 时写栈→崩；改 naked 入口设栈后 call kmain32）；② net32.rs virtio-net legacy 驱动（移植主内核修复经验：QueueAlign+QueuePFN、vring 布局 desc@0/avail@128/used@4096、rx 预填充+notify kick、TX used 回收、GUEST_PAGE_SIZE(0x028)=4096、QueueReady(0x044)=1 对齐主内核序列）；③ 实测：virtio-net up (slot 7)、vring 地址 4096 对齐确认（TX_MEM 0x80002000/RX_MEM 0x80007000）、ARP request 提交 ok；**QEMU -trace 证据（2026-10-08 深查）**：设备侧收到全部 guest 写序列（status 0x3/0xb/0xf、PFN 0x80002/0x80007、GUEST_PAGE_SIZE 0x1000、QueueAlign 0x1000、QueueNumMax 读×2、notify q1 ×8+q0 ×2、MAC 读 0x100-0x105）——**driver 序列完全正确但设备 used ring 零推进**；force-legacy=true 无效；DRIVER_OK 后补 rx kick（q0，avail idx=8 已预填充）亦无效；**InterruptStatus 读回恒 0（2026-10-08 三轮）——设备对 notify 完全静默，QEMU 6.2 riscv32 virtio legacy 深坑实锤**；**下一诊断轴**：modern v2 接口实测（先读 version 确认）/ QEMU 版本升级对比 / qemu riscv32 virtio-mmio notify 源码级深查；④ riscv32imc 无 A 扩展→无 AtomicU64/AtomicU32::fetch_add（MacCell UnsafeCell+Sync、普通计数替代）；⑤ 64KB QueueMem 静态 bss（.as_mut_ptr() 等引用路径全部 addr_of_mut! 裸指针化，Rust 2024 static mut 规则）
- **Runtime logging policy**: All diagnostic logs print unconditionally (no rate-limiting or "first N" counting). Logs are written to UART serial output; redirect to file and grep/filter offline for analysis. Adding back `if count < N` guards is forbidden.
- **Runtime logging policy**: All diagnostic logs print unconditionally (no rate-limiting or "first N" counting). Logs are written to UART serial output; redirect to file and grep/filter offline for analysis. Adding back `if count < N` guards is forbidden.
- **Runtime logging policy**: All diagnostic logs print unconditionally (no rate-limiting or "first N" counting). Logs are written to UART serial output; redirect to file and grep/filter offline for analysis. Adding back `if count < N` guards is forbidden.

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
| `docs/agent/network.md` | smoltcp network stack, Device adapter, socket syscalls, QEMU net config |

## Testing

- **182 QEMU integration tests** via `make test` — runs in-kernel test suite in QEMU (measured 2026-10-08)
- **Test mode**: `make test` internally builds with `cargo +nightly build --release -p karte-os-kernel --features test_mode --target riscv64gc-unknown-none-elf` (stable cannot compile the x86_64 dep tree; see GOTCHAS)
- **Test framework**: `kernel/src/test.rs` — TAP-style `run_test(name, || bool)` API
- **Test modules**: Each subsystem has `#[cfg(feature = "test_mode")] pub fn run_tests()`
- **CI**: GitHub Actions runs build + lint + test + boot-test + smp-test on every push
- **Coverage (RISC-V, 145 total)**: Syscall (29), FS (17 incl. ext4 large/chunked read regression), Capability (8: P2.1 CapDesc register/transition/names/JSON snapshots), DRT (11: state machine edges + wire + tool table), MCP-CB (8: profile roundtrip + dualpath + call idempotency/timeout), VMM (10), Sched 2.0 (7: class 2 + readyqueue 4 + 200-task stress), PMM (6), Heap (6), Task (5), SpinLock (5), IntSpinLock (5), YieldMutex (4), Trap (4), Sv39 (3), SStatus (2), Frame (2), BlockingMutex (2), arch-misc (7: user/switch/sie/sbi/satp/process/kernel)
- **Total**: RISC-V **145/145** (P1.2 mmap 3 + M1 ext4 read 2 + P2.1 CapDesc 8 + P2.2 DRT 12 + P2.3 MCP-CB 8), x86_64 **138/138** (measured 2026-10-08 after Scheduler 2.0 landed)
- **x86_64**: `make test-x86` runs x86_64 integration tests in QEMU
- **Both**: `make test-all` runs RISC-V + x86_64 tests sequentially

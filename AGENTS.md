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

**CI runs 5 jobs on every push: build, lint (fmt + clippy), test (70 tests), boot-test, smp-test.**
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
- **x86_64 Go binary support**: Go runtime (e.g., xbot-cli-static ~69MB) requires: `mmap` (PROT_NONE reservation + PROT_RW allocation), `mprotect`, `brk`, `openat` (for `/proc/version`, `/etc/...`), `write`. Go panics with "failed to determine kernel version" if `openat` for version info returns ENOENT — need to implement `uname` syscall or fake `/proc/version` to proceed further.
- **Arc<spin::Mutex<FdTable>>**: `Process.fd_table` is `Arc<spin::Mutex<FdTable>>`, not `Option<FdTable>`. CLONE_FILES threads share the same fd table via `Arc::clone()` (POSIX semantics). `fork` deep-copies into a new `Arc`. `with_fd_table` clones the Arc under PROCESS_TABLE lock, then locks the fd_table — NEVER hold PROCESS_TABLE lock while accessing fd_table contents (deadlock risk with nested locks).
- **EPOLLET edge-triggered**: `EpollEntry` tracks `last_revents` per fd. For `EPOLLET` entries, `epoll_wait` only reports when `revents` changes from `last_revents`. Without this, every `epoll_wait` returns EPOLLOUT for writable fds → Go netpoller spins forever. `epoll_ctl MOD` resets `last_revents` to 0.
- **EPOLLERR/EPOLLHUP**: Do NOT unconditionally return EPOLLERR/EPOLLHUP when the caller requests them. Only report on real errors or when fd is closed/invalid. Unconditional reporting causes Go's netpoller to loop infinitely.
- **x86_64 xbot testing**: xbot-cli-static creates `.xbot/` directory and files on first run. No pre-existing files needed on disk.
- **x86_64 copy_kernel_mappings huge pages**: Only copy PDP entries with PS=0 (PD table pointers). Skip 1GB huge pages (PS=1) which map MMIO (LAPIC 0xFEE00000, IOAPIC 0xFEC00000). These must NOT be user-accessible — kernel accesses MMIO via `with_kernel_cr3()`.
- **x86_64 user page table MMIO**: LAPIC, IOAPIC, and PCI MMIO are NOT mapped into user page tables. Any user-space access to these addresses triggers PF (by design). Go runtime should never access MMIO directly.
- **Linux syscall compatibility layer**: `linux.rs` translates Linux syscall numbers to KarteOS native syscalls. Key implemented: `uname(122)` (fake "Linux 6.1.0"), `getcwd(79)`, `sysinfo(99)`, `gettimeofday(96)`, `sched_getaffinity(204)`, `clock_gettime(228)`, `mprotect(10)`, `fstat(5)` (via `linux_fstat` in mod.rs), `lseek(8)`. Go runtime depends on these for initialization. x86_64 syscall numbers must match standard Linux x86_64 ABI (NOT RISC-V numbers).
- **x86_64 `int 0x80` vs `syscall` instruction**: Two separate dispatch paths. `int 0x80` → KarteOS native syscalls (mod.rs `dispatch_inner`). `syscall` instruction (MSR LSTAR) → Linux compat layer (mod.rs `dispatch_linux_syscall`). Go uses `syscall` instruction exclusively. The two paths have different syscall number spaces — no conflict between KarteOS number 5 (SYS_GETPID via int 0x80) and Linux number 5 (fstat via syscall).
- **x86_64 SYSCALL uses TSS.RSP0**: `syscall_entry` reads per-task kernel stack from TSS.RSP0 (via `TSS_RSP0_ADDR` pointer), NOT from a global `SYSCALL_KSP`. This is critical because Timer ISR can preempt during CPL=0 SYSCALL execution (IST=0 → no stack stack). Per-task stacks prevent corruption when `__switch()` saves/restores SPs. `TSS_RSP0_ADDR` is a `#[unsafe(no_mangle)]` global pointing to the TSS RSP0 field.
- **x86_64 SYSCALL rbx is per-frame, NOT global**: Original user RBX is pushed onto the syscall's own kernel stack frame (not a global variable). If a syscall triggers `schedule()` and another task's syscall runs, the global would be overwritten. The return path restores RBX from `[rsp - 88]` (relative to the iretq frame), ensuring each syscall restores its own saved value.
- **x86_64 ISR stubs MUST be `#[unsafe(naked)]`**: Keyboard (IRQ1) and COM1 UART (IRQ4) ISR stubs use `naked_asm!` and save/restore ALL 15 GP registers. Non-naked functions get a compiler prologue that only saves rax, and the old code only saved callee-saved registers. This clobbers caller-saved registers (especially rax = syscall return value) when the ISR fires between a SYSCALL return and userspace reading RAX. All ISRs that can interrupt user code must preserve the full register state.
- **x86_64 ext4 sector write-through cache**: `KarteBlockDevice` in `ext4_x86_64.rs` maintains a sector-level write-through cache (`SECTOR_CACHE`, 2048 entries). ext4_rs has no in-memory block cache; without this cache, `write_offset`'s read-modify-write at sector granularity causes bitmap/inode/bgdt updates to clobber each other when sharing the same physical sector. The cache ensures write-after-write consistency: `write_offset` writes to disk AND cache, `read_offset` checks cache first.
- **mmap lazy allocation + VMA tracking**: All MAP_ANONYMOUS mmap creates VMA entries (start/end/prot) but does NOT allocate physical frames. The PF handler lazily allocates zeroed frames on first access, validated against the VMA table. PROT_NONE mappings refuse PF allocation. This is the standard Linux behavior; Go relies on it for `sysReserve`→`sysMap`→`sysUsed`→`sysUnused` lifecycle.
- **madvise MADV_DONTNEED decommits**: MADV_DONTNEED/MADV_FREE releases physical frames (removes PTEs, frees frames via `unmap_user`). MADV_POPULATE/WILLNEED pre-allocates frames. Go's `sysUnused` calls MADV_DONTNEED to release memory; `sysMap` re-commits via mmap(MAP_FIXED). The VMA entry persists across commit/decommit cycles.
- **High-freq debug logs removed**: All runtime console_println calls for [sched], [sleep], [epoll], [ext4wr], [vfs_wr], [openat], [wr] fd=, etc. have been removed. Only initialization and error logs remain. Adding debug logs back will significantly slow down xbot due to UART serial I/O bottleneck.

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

- **59 QEMU integration tests** via `make test` — runs in-kernel test suite in QEMU
- **Test mode**: `cargo build --release --features test_mode` compiles test kernel
- **Test framework**: `kernel/src/test.rs` — TAP-style `run_test(name, || bool)` API
- **Test modules**: Each subsystem has `#[cfg(feature = "test_mode")] pub fn run_tests()`
- **CI**: GitHub Actions runs build + lint + test + boot-test + smp-test on every push
- **Coverage**: PMM (6), VMM (6), Heap (6), FS (15), SpinLock (5), IntSpinLock (5), Mutex (6), Task (6), Syscall (15) = **69 tests** (RISC-V core)
- **Architecture tests**: RISC-V (15) + x86_64 (22) = **37 arch-specific tests**
- **Total**: RISC-V 96/96, x86_64 102/103 (1 PMM test fails on x86_64 — different physical memory layout)
- **x86_64**: `make test-x86` runs x86_64 integration tests in QEMU
- **Both**: `make test-all` runs RISC-V + x86_64 tests sequentially

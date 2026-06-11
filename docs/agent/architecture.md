# Architecture

## Boot Flow

```
QEMU (virt machine, rv64)
  → OpenSBI (M-mode firmware, QEMU bundled)
    → _start @ 0x80200000 (arch/riscv64/entry.S)
      → Clear BSS (_sbss .. _ebss)
      → Set SP = _boot_stack_top
      → call kmain(hartid, dtb_ptr)
        → 10-phase init (see main.rs)
        → Load user ELF → create process → first_enter_user
        → User runs in U-mode, traps handled by trap_handler
        → sys_exit → schedule_exit → shutdown if last process
```

## Memory Layout

```
0x0000_0000 .. 0x0C00_0000  — Unmapped
0x0C00_0000 .. 0x0C40_0000  — PLIC MMIO (identity mapped, KRW)
0x1000_0000 .. 0x1000_1000  — UART MMIO (ns16550a, identity mapped, KRW)
0x1000_1000 .. 0x1000_3000  — VirtIO MMIO devices (identity mapped, KRW)
0x8020_0000 .. _ekernel     — Kernel code + data + BSS + boot stack (KRWX)
_ekernel      .. 0x8020_0000+128M — Managed physical memory (via PMM)
0x0000_1000              — User ELF load address (entry point)
0x7FF0_0000 .. 0x8000_0000 — User stack (grows down, ~1MB)
```

## Multi-Process Architecture

Each process has:
- **Independent Sv39 page table** with kernel mappings copied in via `copy_kernel_mappings()`
- **Kernel stack** (4 pages = 16KB) allocated from PMM
- **User stack** (mapped at 0x7FF00000..0x80000000)
- **TrapContext** (280 bytes) on kernel stack for U-mode trap entry/exit
- **Process struct** (pid, entry, brk, page_table_root, stack pointers)

### Process Lifecycle

```
sys_spawn(prog_id) → Process::from_elf() → add_process() → add_user_process()
  → Child gets Ready state in scheduler
  → Parent gets child PID as return value

Timer interrupt → schedule() → __switch() → Round-Robin to next Ready task
  → trap_handler restores child's satp (in Rust, not assembly)
  → sret to child's U-mode entry point

sys_exit(code) → schedule_exit() → mark Exited → switch to next Ready task
  → If no Ready tasks remain → SBI shutdown
```

### satp Switching Strategy

**Critical design decision**: satp (user page table) is restored in Rust `trap_handler`,
NOT in `trap_entry.S`. The assembly return path stays simple and fast (no sfence.vma).
Only when satp actually changed (context switch) does the Rust code write satp + sfence.vma.
For single-process, this is a no-op — no sfence.vma overhead.

## 10-Phase Init Sequence

| Phase | Subsystem | Key Calls |
|-------|-----------|-----------|
| 1 | UART | `Uart::new(0x10000000).init()` |
| 2 | Trap | `trap::init()` — set stvec |
| 3 | SMP | `smp::init_bsp(hartid)` — store hart in tp |
| 4 | PMM | `pmm::init()` — bitmap allocator |
| 5 | VMM | `vmm::init()` — Sv39 page table + satp |
| 6 | Heap | `heap::init()` — 1MB buddy allocator |
| 7 | VirtIO | `virtio::probe_virtio_devices()` — scan MMIO |
| 8 | FS | `fs::init()` — in-memory filesystem |
| 9 | Timer+PLIC | `enable_timer_interrupt()`, `plic::init()` |
| 10 | Scheduler+User | `sched::init()`, load user ELF, `first_enter_user()` |

## Subsystem Dependencies

```
arch/riscv64/sbi.rs (console output, SBI calls)
  ↑ used by all other modules via crate::arch::sbi

driver/uart.rs (serial console)
driver/virtio.rs → depends on mm/pmm (DMA buffers), mm/vmm (MMIO mapping)
driver/fs.rs     → depends on mm/heap (Vec, String via alloc)

arch/riscv64/trap.rs     → depends on arch::sbi, sched/ (timer → schedule), process/ (satp switch)
arch/riscv64/plic.rs     → depends on driver/uart (interrupt handler)
arch/riscv64/smp.rs      → depends on mm/pmm (stack alloc), arch::trap, arch::plic
arch/x86_64/cr3.rs       → RAII CR3 guards (enter_kernel_cr3 → Cr3Guard auto-restore)
arch/x86_64/user_return.rs → Typed FsBase, UserReturnState for user-return paths

mm/pmm.rs        → standalone (uses linker symbols _ekernel)
mm/vmm.rs        → depends on mm/pmm (page table allocation)
mm/heap.rs       → depends on mm/pmm (heap pages)
mm/addr.rs       → standalone typed address newtypes (PhysAddr, UserVirtAddr, etc.)
mm/frame.rs      → depends on mm/pmm (frame alloc/dealloc in Drop)
mm/page_table.rs → standalone (WalkResult enum, Level markers)
mm/address_space.rs → depends on mm/vma (VMA operations), mm/addr (PhysAddr)
mm/diagnostics.rs   → depends on mm/vmm (PageTable walk), mm/addr, mm/page_table

process/         → depends on mm/pmm (page tables + stacks), mm/vmm (user mapping)
sched/           → depends on mm/pmm (task stacks), process/ (task→process mapping), sync/spinlock
syscall/         → depends on sched (getpid, exit, spawn), process/ (current, from_elf), arch::sbi (write)

main.rs          → orchestrates all subsystems in correct order
platform.rs      → architecture-specific constants (MMIO bases, memory layout)
```

## Crate Dependencies

RISC-V specific dependencies are gated by target architecture in `kernel/Cargo.toml`:

```toml
[dependencies]
buddy_system_allocator = "0.13"
spin = "0.12"
bitflags = "2.11"
virtio-drivers = { version = "0.13.0", features = ["alloc"] }
starry-fatfs = { version = "0.4.1-preview.2", default-features = false, features = ["alloc", "lfn"] }
log = { version = "0.4", default-features = false }
ext4_rs = { path = "vendor/ext4_rs" }

[target.'cfg(target_arch = "riscv64")'.dependencies]
riscv = "0.16"                     # CSR access, register structs
riscv-rt = "0.17"
sbi = "0.3.0"                      # SBI timer, system_reset, hsm
```

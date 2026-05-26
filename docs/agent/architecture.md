# Architecture

## Boot Flow

```
QEMU (virt machine, rv64)
  → OpenSBI (M-mode firmware, QEMU bundled)
    → _start @ 0x80200000 (entry.S)
      → Clear BSS (_sbss .. _ebss)
      → Set SP = _boot_stack_top
      → call kmain(hartid, dtb_ptr)
        → 10-phase init (see main.rs)
        → Enable S-mode interrupts (sstatus.SIE)
        → wfi loop (scheduler runs via timer interrupts)
```

## Memory Layout

```
0x0000_0000 .. 0x0C00_0000  — Unmapped
0x0C00_0000 .. 0x0C40_0000  — PLIC MMIO (identity mapped, KRW)
0x1000_0000 .. 0x1000_1000  — UART MMIO (ns16550a, identity mapped, KRW)
0x1000_1000 .. 0x1000_3000  — VirtIO MMIO devices (identity mapped, KRW)
0x8020_0000 .. _ekernel     — Kernel code + data + BSS + boot stack (KRWX)
_ekernel      .. 0x8020_0000+128M — Managed physical memory (via PMM)
```

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
| 9 | Net | `net::test_net()` — probe VirtIO Net |
| 10 | Timer+PLIC+Sched | `enable_timer_interrupt()`, `plic::init()`, `smp::start_secondary_harts()`, `sched::init()` |

## Subsystem Dependencies

```
sbi.rs (console output)
  ↑ used by all other modules

driver/uart.rs (serial console)
driver/virtio.rs → depends on mm/pmm (DMA buffers), mm/vmm (MMIO mapping)
driver/net.rs    → depends on mm/pmm (VirtQueue buffers)
driver/fs.rs     → depends on mm/heap (Vec, String via alloc)

arch/trap.rs     → depends on sbi.rs, sched/ (timer → schedule)
arch/plic.rs     → depends on driver/uart (interrupt handler)
arch/smp.rs      → depends on mm/pmm (stack alloc), arch/trap, arch/plic

mm/pmm.rs        → standalone (uses linker symbols _ekernel)
mm/vmm.rs        → depends on mm/pmm (page table allocation)
mm/heap.rs       → depends on mm/pmm (heap pages)

sched/           → depends on mm/pmm (task stacks), sync/spinlock
syscall/         → depends on sched (getpid, exit), sbi (write)

main.rs          → orchestrates all subsystems in correct order
```

## Crate Dependencies

```toml
riscv = "0.12"                    # CSR access, register structs
riscv-rt = "0.13"                 # (minimal use — custom entry.S)
sbi-rt = { version = "0.0.4", features = ["legacy"] }  # SBI ecall wrappers
buddy_system_allocator = "0.11"   # Kernel heap
virtio-drivers = { version = "0.13.0", features = ["alloc"] }  # VirtIO
spin = "0.9"                      # SpinLock
bitflags = "2.0"                  # PTE flags
```

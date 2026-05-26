# SMP (Symmetric Multiprocessing)

## Overview

- **File**: `kernel/src/arch/smp.rs`
- **Max harts**: 8
- **Boot**: BSP (any hart, chosen by OpenSBI) → secondary harts via SBI

## Hart States

```rust
pub enum HartState {
    Stopped,   // Not yet started
    Starting,  // SBI hart_start issued
    Running,   // Online and executing
}
```

## BSP Initialization

Called from `kmain`:
1. Store hartid in `tp` register (`asm!("mv tp, {}", in(reg) hartid)`)
2. Mark hart as Running in HART_DATA table
3. Set ACTIVE_HARTS = 1

## Secondary Hart Startup

Called from main.rs after all init:
1. Allocate 4 pages stack per secondary hart via PMM
2. Call `sbi_rt::hart_start(hartid, entry_addr, opaque)`
3. Secondary harts jump to `secondary_hart_entry()`

### secondary_hart_entry(hartid)
1. Store hartid in `tp`
2. Set up stack from pre-allocated area
3. Init trap vector for this hart
4. Enable timer interrupts
5. Init PLIC for this hart
6. Enable S-mode interrupts (`sstatus::set_sie`)
7. Enter `wfi` loop

## Per-Hart Resources

Each hart needs its own:
- Stack (4 pages from PMM)
- Trap vector (same handler, different stack)
- PLIC context (per-hart enable/threshold/claim)
- Timer interrupt (independent scheduling)

## current_hart()

```rust
pub fn current_hart() -> usize {
    // Read from tp register (set during init)
    asm!("mv {}, tp", out(reg) hartid)
}
```

## Limitations

- No lock-free per-hart data structures yet (uses SpinLock)
- Secondary hart startup via SBI may fail (returns SbiRet error)
- No hart hotplug/hot-unplug
- No IPI (Inter-Processor Interrupts) yet

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

## SMP Scheduling (2026-05-27)

### Per-hart State

Each hart has its own local state stored in per-hart atomic arrays:
- `CURRENT_PROCESS[hartid]`: process index in PROCESS_TABLE
- `CURRENT_PAGE_TABLE_ROOT[hartid]`: PPN for satp restore in trap_handler
- `HART_CURRENT_TASK[hartid]`: task ID for current_task_id()

`hartid()` reads from `tp` register (set by `init_bsp` / trampoline assembly).
Has bounds check — falls back to 0 if tp is uninitialized (test mode safety).

### Secondary Hart Entry

`secondary_hart_entry(hartid)`:
1. Verify stack allocated
2. `trap::init()` — set stvec for this hart
3. Enable timer interrupt + set 10ms timer
4. Init PLIC context for this hart
5. Mark HartState::Running, increment ACTIVE_HARTS
6. Set SIE (enable interrupts)
7. Enter scheduling loop: `loop { schedule(); wfi; }`

### Scheduler Design

Global `SCHEDULER.lock()` protects the shared task pool. `sched.current` (in Scheduler struct) is the global current task, protected by the lock. Each hart calls `schedule()` from its timer interrupt handler, which:
1. Acquires SCHEDULER lock
2. Finds next Ready task (Round-Robin)
3. Updates task states + CURRENT_PAGE_TABLE_ROOT
4. Drops lock
5. Calls `__switch()`

Only one hart executes `__switch` at a time (lock-protected).

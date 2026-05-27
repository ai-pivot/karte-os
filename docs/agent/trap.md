# Trap Handling

## Overview

- **File**: `kernel/src/arch/trap.rs`, `kernel/src/arch/trap_entry.S`
- **Mode**: Direct (stvec points to single handler)
- **Dual-path**: S-mode vs U-mode traps distinguished by sscratch

## Trap Context (TrapContext)

```rust
pub struct TrapContext {
    pub x: [usize; 32],   // General-purpose registers x0-x31 (256 bytes)
    pub sstatus: usize,    // Supervisor Status (8 bytes)
    pub sepc: usize,       // Supervisor Exception PC (8 bytes)
    pub sscratch: usize,   // User sp if from U-mode, else 0 (8 bytes)
}  // Total: 280 bytes
```

### Register Save Order on Stack

```
sp+8   : x1 (ra)
sp+16  : x2 (sp) — user_sp from sscratch (U-mode) or 0 (S-mode)
sp+24  : x3 (gp)
sp+32  : x4 (tp)
...
sp+248 : x31
sp+256 : sstatus
sp+264 : sepc
sp+272 : sscratch (user sp if from U-mode)
```

## Dual-Path Trap Entry

The trap entry uses `sscratch` to distinguish trap source:
- **S-mode trap**: sscratch=0, use S-mode sp directly
- **U-mode trap**: sscratch=kernel_sp (set by previous U-mode return), swap sp with sscratch

```
trap_entry:
  csrrw sp, sscratch, sp    # swap sp with sscratch
  bnez sp, .Ltrap_from_user # if old_sscratch != 0 → U-mode
  # ... S-mode path ...
.Ltrap_from_user:
  # ... U-mode path ...
```

## Trap Flow (U-mode)

```
Trap occurs in U-mode → trap_entry (assembly)
  → Swap sp (kernel_sp) with sscratch (user_sp)
  → Allocate 280-byte TrapContext on kernel stack
  → Save x[0..31], sstatus, sepc, sscratch to stack
  → Call trap_handler(&mut TrapContext)
    → Set SSTATUS.SUM (S-mode can access U-mode pages)
    → Read scause, stval
    → Match on trap type:
      SupervisorTimer → handle_timer() → schedule() → __switch()
      SupervisorExternal → plic::handle_interrupt()
      UserEnvCall → syscall dispatch (sepc += 4)
      LoadPageFault → lazy page allocation (brk/mmap regions)
      Other Exception → log + skip_compressed_insn()
    → Restore satp for current process (multi-process support)
    → Clear SSTATUS.SUM
  → Restore sepc, sstatus, x[0..31]
  → Pop TrapContext frame
  → Swap sp (user_sp) with sscratch (kernel_sp)
  → sret to U-mode
```

## satp Switching (Multi-Process)

**Current state**: satp is NOT restored in `trap_handler` or `trap_entry.S`.

The `TrapContext` struct does NOT contain a satp field. The `__switch` assembly
does NOT touch satp. After a context switch via timer interrupt → schedule() →
`__switch`, the new task resumes in Rust code still running on the OLD process's
page table. satp only gets switched at two points:
1. `first_enter_user()` — sets satp + sfence.vma before the initial sret to U-mode
2. `sys_spawn()` — prepares the new process but does NOT switch satp (the first
   U-mode trap return from `__switch` resumes on whatever satp was active)

The scheduler updates `CURRENT_PAGE_TABLE_ROOT` (AtomicUsize) in `schedule()`
and `schedule_exit()` via `process::set_current_page_table_root()`, but this
value is only consumed by `get_current_user_pt()` for page table lookup, NOT
for restoring the hardware satp register.

**Known gap**: After `__switch` in `schedule()`, the kernel runs on the previous
process's page table until the next U-mode return or a new satp write. This
works because all user page tables include identical kernel identity mappings
(see `copy_kernel_mappings()` in process/mod.rs), so kernel code/data access
is unaffected. However, any kernel code that reads U-mode pages via the wrong
page table will see the wrong process's memory.

## Timer Interrupt

- **Clock**: QEMU virt ACLINT mtimer @ 10 MHz
- **Quantum**: 10ms = 100,000 ticks
- **Setup**: `sbi::timer::set_timer(next)` for next deadline
- **Enable**: `sie::set_stimer()` + `sstatus::set_sie()`

## Syscall Handling

UserEnvCall (exception code 8):
- Syscall number in `a7` (x[17])
- Arguments in `a0-a5` (x[10]-x[15])
- Return value in `a0` (x[10])
- `sepc += 4` to skip ecall instruction

## stvec Setup

```rust
stvec::write(trap_entry as *const () as usize, stvec::TrapMode::Direct);
```

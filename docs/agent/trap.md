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

**Design**: satp is restored in Rust `trap_handler`, NOT in `trap_entry.S`.

Rationale: `sfence.vma` in the assembly return path causes timing-sensitive bugs
(expensive TLB flush between sstatus restore and sret). Instead, the Rust
trap_handler checks if satp changed and only writes satp + sfence.vma when
necessary (after a context switch). For single-process, this is a no-op.

```rust
// In trap_handler, before returning:
if from_user {
    if let Some(proc) = process::current() {
        let expected_satp = (8usize << 60) | proc.page_table_root;
        let current_satp = read_csr!(satp);
        if current_satp != expected_satp {
            write_csr!(satp, expected_satp);
            sfence.vma();
        }
    }
}
```

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

# Trap Handling

## Overview

- **File**: `kernel/src/arch/trap.rs`
- **Mode**: Direct (stvec points to single handler)
- **Assembly**: `global_asm!()` embedded in trap.rs

## Trap Context (TrapContext)

```rust
pub struct TrapContext {
    pub x: [usize; 32],   // General-purpose registers x0-x31 (256 bytes)
    pub sstatus: usize,    // Supervisor Status (8 bytes)
    pub sepc: usize,       // Supervisor Exception PC (8 bytes)
}  // Total: 272 bytes, stack frame: 280 bytes (aligned)
```

### Register Save Order on Stack

```
sp+8   : x1 (ra)
sp+24  : x3 (gp) — skip x2 (sp), restored via addi
sp+32  : x4 (tp)
...
sp+248 : x31
sp+256 : sstatus
sp+264 : sepc
```

## Trap Flow

```
Trap occurs → trap_entry (assembly)
  → Save x[0..31], sstatus, sepc to stack
  → Call trap_handler(&mut TrapContext)
    → Read scause, stval
    → Match on trap type:
      SupervisorTimer → handle_timer() → schedule()
      SupervisorExternal → plic::handle_interrupt()
      UserEnvCall → syscall dispatch
      Other Exception → log + skip (sepc += 4)
  → Restore sstatus, sepc, x[0..31]
  → sret
```

## Timer Interrupt

- **Clock**: QEMU virt ACLINT mtimer @ 10 MHz
- **Quantum**: 10ms = 100,000 ticks
- **Counter**: `AtomicUsize` (TIMER_TICKS), prints every 100 ticks (1s)
- **Setup**: `sbi_rt::set_timer(next)` for next deadline
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

# Scheduler

## Overview

- **Files**: `kernel/src/sched/mod.rs`, `task.rs`, `switch.S`
- **Algorithm**: Round-Robin with timer preemption (10ms quantum)
- **Max tasks**: 64

## Task Structures

### TaskContext (callee-saved registers)

```rust
pub struct TaskContext {
    pub ra: usize,    // Return address (where to jump on restore)
    pub sp: usize,    // Stack pointer
    pub s: [usize; 12], // s0-s11 (callee-saved)
}
```

- `ra` = task entry point (set via `goto(entry, stack_top)`)
- `sp` = top of allocated task stack (grows downward)
- 13 registers × 8 bytes = 104 bytes per context

### TaskControlBlock

```rust
pub struct TaskControlBlock {
    pub context: TaskContext,
    pub tid: usize,
    pub state: TaskState,  // Ready | Running | Blocked | Exited
}
```

## Context Switch (switch.S)

```
__switch(current_sp: &mut usize, next_sp: usize)
```

1. Save callee-saved regs (ra, s0-s11) to current stack
2. Store current SP to `*current_sp`
3. Load next SP from `next_sp`
4. Restore callee-saved regs from next stack
5. `ret` — jumps to restored `ra` (task entry or previous PC)

### Stack Frame (104 bytes)

```
sp+0   : ra
sp+8   : s0/fp
sp+16  : s1
...
sp+96  : s11
```

## Round-Robin Algorithm

1. Called from `handle_timer()` every 10ms
2. Find next Ready task after current
3. If no Ready task found, stay on current
4. Save current task's SP via `__switch`
5. Restore next task's SP — execution continues in next task

## Demo Tasks

Three kernel tasks (A, B, C) created at init:
- Each gets 4 pages (16KB) stack from PMM
- Count up and print every 500000 iterations
- Run in Round-Robin rotation via timer interrupts

## Scheduler API

```rust
pub fn init()                    // Create demo tasks, log count
pub fn schedule()                // Switch to next ready task (called from timer)
pub fn current_task_id() -> usize // Get current task's TID
pub fn mark_current_exited()     // Mark current task as Exited (for sys_exit)
```

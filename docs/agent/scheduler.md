# Scheduler

## Overview

- **Files**: `kernel/src/sched/mod.rs`, `task.rs`, `switch.S`
- **Algorithm**: Round-Robin with timer preemption (10ms quantum)
- **Max tasks**: 64
- **Per-task state**: Each task maps to a process in `PROCESS_TABLE` via `task_to_process[]`

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

## Multi-Process Support

The scheduler maps tasks to processes. Each task's kernel stack contains a `TrapContext` (280 bytes)
at the top, which stores the complete U-mode register state. When a task is first created:

1. `add_user_process()` allocates 280 bytes on the kernel stack for TrapContext
2. Sets sepc (user entry), sstatus (SPIE=1), sscratch (user sp), x[2] (kernel sp)
3. The task's saved SP points to the TrapContext base
4. When `__switch` restores this SP, trap_entry.S returns to U-mode via the TrapContext

### Task → Process Mapping

- `task_to_process[tid]` maps scheduler task ID to `PROCESS_TABLE` index
- On context switch, `process::set_current_index()` is called to update the current process
- This ensures syscalls like `sys_getpid` and `sys_brk` access the correct process state

## Round-Robin Algorithm

1. Called from `handle_timer()` every 10ms
2. Find next Ready task after current
3. If no Ready task found, stay on current
4. Mark current Ready, mark next Running
5. Call `process::set_current_index()` for next task
6. Save current task's SP via `__switch`
7. Restore next task's SP — execution continues in next task's trap_handler return path

## Process Exit

`schedule_exit()` handles the last-process and multi-process cases:
- Mark current task as Exited
- Find next Ready task
- If found: switch to it (multi-process)
- If not found: SBI shutdown (last process exited)

## Scheduler API

```rust
pub fn init()                                          // Initialize scheduler
pub fn schedule()                                      // Round-Robin switch (timer interrupt)
pub fn schedule_exit()                                 // Exit current task, switch or shutdown
pub fn current_task_id() -> usize                      // Get current task's TID
pub fn add_user_process(entry, user_sp, kernel_sp, satp, proc_idx) -> Option<usize>
                                                       // Register a new user process
pub fn current_brk() -> usize                          // Delegate to process module
pub fn set_current_brk(addr: usize)                    // Delegate to process module
```

// kernel/src/sched/mod.rs — Round-Robin task scheduler with context switching

pub mod task;

use core::arch::global_asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sync::spinlock::SpinLock;
use task::{TaskContext, TaskControlBlock, TaskState};

// Embed the context-switch assembly
global_asm!(include_str!("switch.S"));

const MAX_TASKS: usize = 64;

unsafe extern "C" {
    fn __switch(current_sp: *mut usize, next_sp: *const usize);
}

// Per-task saved stack pointers, accessible without locking the scheduler.
// Written by __switch (assembly) and during task creation (Rust).
// AtomicUsize is #[repr(transparent)] over usize, so casting to *mut usize is safe.
static TASK_SPS: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(0) }; MAX_TASKS];

struct Scheduler {
    tasks: [Option<TaskControlBlock>; MAX_TASKS],
    /// Map task_id → process index in PROCESS_TABLE
    task_to_process: [usize; MAX_TASKS],
    current: usize,
    count: usize,
}

static SCHEDULER: SpinLock<Scheduler> = SpinLock::new(Scheduler {
    tasks: [const { None }; MAX_TASKS],
    task_to_process: [0; MAX_TASKS],
    current: 0,
    count: 0,
});

/// Initialize the scheduler
pub fn init() {
    crate::console_println!("[sched] Initialized");
}

/// Called from timer interrupt handler to perform Round-Robin scheduling.
///
/// This function must be called with interrupts disabled (SIE=0),
/// which is guaranteed because it's called from the trap handler.
pub fn schedule() {
    // Lock the scheduler to update task states and find the next task
    let (current, next) = {
        let mut sched = SCHEDULER.lock();

        if sched.count <= 1 {
            return;
        }

        let current = sched.current;

        // Round-Robin: find next Ready task after current
        let mut next = current;
        for i in 1..sched.count {
            let candidate = (current + i) % sched.count;
            if sched.tasks[candidate]
                .as_ref()
                .map_or(false, |t| t.state == TaskState::Ready)
            {
                next = candidate;
                break;
            }
        }

        // No other ready task — keep running current
        if next == current {
            return;
        }

        // Update task states
        if let Some(ref mut t) = sched.tasks[current] {
            t.state = TaskState::Ready;
        }
        if let Some(ref mut t) = sched.tasks[next] {
            t.state = TaskState::Running;
        }

        sched.current = next;

        // Update process module's current process index
        let next_proc_idx = sched.task_to_process[next];
        crate::process::set_current_index(next_proc_idx);

        (current, next)
    }; // SpinLock dropped here — safe because interrupts are disabled

    // Get raw pointers to the saved stack pointers in TASK_SPS.
    // AtomicUsize has the same layout as usize, so the cast is valid.
    let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
    let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;

    // Perform context switch
    unsafe {
        __switch(cur_ptr, nxt_ptr);
    }
}

/// Get current task's ID
pub fn current_task_id() -> usize {
    SCHEDULER.lock().current
}

/// Mark the current task as exited and switch to the next ready task.
/// If no ready tasks remain, shut down the system.
pub fn schedule_exit() {
    let (has_next, current, next) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;

        // Mark current as exited
        if let Some(ref mut t) = sched.tasks[current] {
            t.state = TaskState::Exited;
        }

        // Find next Ready task
        let mut next = None;
        for i in 1..sched.count {
            let candidate = (current + i) % sched.count;
            if sched.tasks[candidate]
                .as_ref()
                .map_or(false, |t| t.state == TaskState::Ready)
            {
                next = Some(candidate);
                break;
            }
        }

        match next {
            Some(n) => {
                if let Some(ref mut t) = sched.tasks[n] {
                    t.state = TaskState::Running;
                }
                sched.current = n;
                let next_proc_idx = sched.task_to_process[n];
                crate::process::set_current_index(next_proc_idx);
                (true, current, n)
            }
            None => (false, current, 0),
        }
    };

    if !has_next {
        // No more runnable tasks — shut down
        crate::console_println!("[sched] All processes exited, shutting down");
        crate::sbi::shutdown();
    }

    // Perform context switch to next task
    let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
    let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
    unsafe {
        __switch(cur_ptr, nxt_ptr);
    }
}

/// Mark the current task as exited (without scheduling).
pub fn mark_current_exited() {
    let mut sched = SCHEDULER.lock();
    let current = sched.current;
    if let Some(ref mut t) = sched.tasks[current] {
        t.state = TaskState::Exited;
    }
}

/// Add a user process to the scheduler.
/// `process_idx` is the index in PROCESS_TABLE.
/// Returns the task ID or None on failure.
pub fn add_user_process(
    entry: usize,
    user_stack_top: usize,
    kernel_stack_top: usize,
    _user_satp: usize,
    process_idx: usize,
) -> Option<usize> {
    let mut sched = SCHEDULER.lock();
    if sched.count >= MAX_TASKS {
        return None;
    }

    let tid = sched.count;

    // Set up the kernel stack with a TrapContext for the first U-mode entry
    // TrapContext layout: x[32] + sstatus + sepc + sscratch = 280 bytes
    let trap_ctx_top = kernel_stack_top;
    let trap_ctx_base = trap_ctx_top - 280;

    unsafe {
        let ctx = trap_ctx_base as *mut usize;
        // Zero everything
        for i in 0..35 {
            *ctx.add(i) = 0;
        }
        // x[2] = kernel_stack_top (sp during kernel trap handling)
        *ctx.add(2) = kernel_stack_top;
        // sstatus at offset 256/8 = 32: SPP=0, SPIE=1
        *ctx.add(32) = 0x20;
        // sepc at offset 264/8 = 33: entry point
        *ctx.add(33) = entry;
        // sscratch at offset 272/8 = 34: user sp
        *ctx.add(34) = user_stack_top;
        // Note: satp is NOT stored in TrapContext.
        // It's restored by trap_handler (in Rust) before returning to assembly.
    }

    // The task's saved SP points to the trap context
    TASK_SPS[tid].store(trap_ctx_base, Ordering::Relaxed);

    // Create TCB
    let mut tcb = TaskControlBlock::new(tid);
    tcb.context = TaskContext::goto(entry, kernel_stack_top);
    sched.tasks[tid] = Some(tcb);
    sched.task_to_process[tid] = process_idx;
    sched.count += 1;

    Some(tid)
}

/// Get current process brk (delegates to process module).
pub fn current_brk() -> usize {
    crate::process::current_brk()
}

/// Set current process brk (delegates to process module).
pub fn set_current_brk(addr: usize) {
    crate::process::set_current_brk(addr);
}

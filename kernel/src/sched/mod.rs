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

/// Maximum harts (cores) supported for SMP
const MAX_HARTS: usize = 8;

/// Per-hart current task ID (-1 = no task assigned)
static HART_CURRENT_TASK: [AtomicUsize; MAX_HARTS] =
    [const { AtomicUsize::new(usize::MAX) }; MAX_HARTS];

/// Helper: get current hart ID
fn hartid() -> usize {
    let h = crate::arch::smp::current_hart();
    if h >= MAX_HARTS { 0 } else { h }
}

struct Scheduler {
    tasks: [Option<TaskControlBlock>; MAX_TASKS],
    /// Map task_id → process index in PROCESS_TABLE
    task_to_process: [usize; MAX_TASKS],
    count: usize,
}

static SCHEDULER: SpinLock<Scheduler> = SpinLock::new(Scheduler {
    tasks: [const { None }; MAX_TASKS],
    task_to_process: [0; MAX_TASKS],
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
    let h = hartid();
    let cur = HART_CURRENT_TASK[h].load(Ordering::Relaxed);

    // No task assigned to this hart — nothing to schedule
    if cur == usize::MAX {
        // Maybe there are Ready tasks in the global pool? Try to pick one.
        let _ = pick_next_task(h, None);
        return;
    }

    // Lock the scheduler to update task states and find the next task
    let (current, next) = {
        let mut sched = SCHEDULER.lock();

        let current = cur;

        if sched.count <= 1 {
            return;
        }

        // Mark current as Ready (it's currently Running)
        if let Some(ref mut t) = sched.tasks[current] {
            if t.state == TaskState::Running {
                t.state = TaskState::Ready;
            }
        }

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
            // Restore Running state since we set it to Ready above
            if let Some(ref mut t) = sched.tasks[current] {
                t.state = TaskState::Running;
            }
            return;
        }

        // Update task states
        if let Some(ref mut t) = sched.tasks[next] {
            t.state = TaskState::Running;
        }

        // Update per-hart tracking
        HART_CURRENT_TASK[h].store(next, Ordering::Relaxed);

        // Update process module's current process index and page table root
        let next_proc_idx = sched.task_to_process[next];
        crate::process::set_current_index(next_proc_idx);
        crate::process::set_current_page_table_root(crate::process::get_page_table_root(
            next_proc_idx,
        ));

        (current, next)
    }; // SpinLock dropped here — safe because interrupts are disabled

    // Get raw pointers to the saved stack pointers in TASK_SPS.
    let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
    let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;

    // Perform context switch
    unsafe {
        __switch(cur_ptr, nxt_ptr);
    }
}

/// Try to pick a Ready task for a hart that has none.
/// Called from schedule() when hart has no current task.
/// Returns the picked task ID if found, None otherwise.
fn pick_next_task(h: usize, _exclude: Option<usize>) -> Option<usize> {
    let mut sched = SCHEDULER.lock();
    for i in 0..sched.count {
        if sched.tasks[i]
            .as_ref()
            .map_or(false, |t| t.state == TaskState::Ready)
        {
            if let Some(ref mut t) = sched.tasks[i] {
                t.state = TaskState::Running;
            }
            HART_CURRENT_TASK[h].store(i, Ordering::Relaxed);
            let proc_idx = sched.task_to_process[i];
            crate::process::set_current_index(proc_idx);
            crate::process::set_current_page_table_root(crate::process::get_page_table_root(
                proc_idx,
            ));
            return Some(i);
        }
    }
    None
}

/// Get current task's ID
pub fn current_task_id() -> usize {
    HART_CURRENT_TASK[hartid()].load(Ordering::Relaxed)
}

/// Mark the current task as exited and switch to the next ready task.
pub fn schedule_exit() {
    let h = hartid();
    let (has_next, current, next) = {
        let mut sched = SCHEDULER.lock();
        let current = HART_CURRENT_TASK[h].load(Ordering::Relaxed);

        if current == usize::MAX {
            crate::console_println!("[sched] Hart {} no task, parking", h);
            loop {
                unsafe { core::arch::asm!("wfi") };
            }
        }

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
                HART_CURRENT_TASK[h].store(n, Ordering::Relaxed);
                let next_proc_idx = sched.task_to_process[n];
                crate::process::set_current_index(next_proc_idx);
                crate::process::set_current_page_table_root(crate::process::get_page_table_root(
                    next_proc_idx,
                ));
                (true, current, n)
            }
            None => (false, current, 0),
        }
    };

    if !has_next {
        let sched = SCHEDULER.lock();
        let alive = sched.tasks[..sched.count]
            .iter()
            .filter(|t| t.as_ref().map_or(false, |t| t.state != TaskState::Exited))
            .count();
        if alive == 0 {
            crate::console_println!("[sched] All processes exited, shutting down");
            crate::sbi::shutdown();
        } else {
            loop {
                unsafe { core::arch::asm!("wfi") };
            }
        }
    }

    // Perform context switch
    let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
    let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
    unsafe {
        __switch(cur_ptr, nxt_ptr);
    }
}

/// Mark the current task as exited (without scheduling).
pub fn mark_current_exited() {
    let h = hartid();
    let cur = HART_CURRENT_TASK[h].load(Ordering::Relaxed);
    if cur == usize::MAX {
        return;
    }
    let mut sched = SCHEDULER.lock();
    if let Some(ref mut t) = sched.tasks[cur] {
        t.state = TaskState::Exited;
    }
}

/// Block the current task and switch to the next ready task.
/// Used by sys_waitpid to put the parent to sleep while waiting for a child.
/// Returns when the task is unblocked (woken up by child's sys_exit).
pub fn schedule_block() {
    let h = hartid();
    let (has_next, current, next) = {
        let mut sched = SCHEDULER.lock();
        let current = HART_CURRENT_TASK[h].load(Ordering::Relaxed);

        if current == usize::MAX {
            // Shouldn't happen — blocked from a hart with no task
            return;
        }

        // Mark current as Blocked
        if let Some(ref mut t) = sched.tasks[current] {
            t.state = TaskState::Blocked;
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
                HART_CURRENT_TASK[h].store(n, Ordering::Relaxed);
                let next_proc_idx = sched.task_to_process[n];
                crate::process::set_current_index(next_proc_idx);
                crate::process::set_current_page_table_root(crate::process::get_page_table_root(
                    next_proc_idx,
                ));
                (true, current, n)
            }
            None => (false, current, 0),
        }
    };

    if !has_next {
        crate::console_println!("[sched] No runnable tasks, shutting down");
        crate::sbi::shutdown();
    }

    // Perform context switch to next task
    let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
    let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
    unsafe {
        __switch(cur_ptr, nxt_ptr);
    }
}

/// Wake up a blocked task by process index.
/// Sets it back to Ready so it will be scheduled on the next timer tick.
pub fn wake_task(proc_idx: usize) {
    let mut sched = SCHEDULER.lock();
    // Find the task with this process index
    for i in 0..sched.count {
        if sched.task_to_process[i] == proc_idx {
            if let Some(ref mut t) = sched.tasks[i] {
                if t.state == TaskState::Blocked {
                    t.state = TaskState::Ready;
                }
            }
            return;
        }
    }
}

/// Remove a task from the scheduler by process index.
/// Used by sys_waitpid after reclaiming a child process.
pub fn remove_task(proc_idx: usize) {
    let mut sched = SCHEDULER.lock();
    // Find the task with this process index
    for i in 0..sched.count {
        if sched.task_to_process[i] == proc_idx {
            sched.tasks[i] = None;
            return;
        }
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

    // Set up the kernel stack with a TrapContext for the first U-mode entry.
    // Stack layout (high → low):
    //   kernel_stack_top
    //     TrapContext (280 bytes): x[0..31], sstatus, sepc, sscratch
    //     __switch frame (104 bytes): ra, s0-s11 for __switch to restore
    //     TASK_SPS[tid] points here ←
    let trap_ctx_base = kernel_stack_top - 280;
    let switch_sp = trap_ctx_base - 104;

    unsafe {
        // Zero both regions
        core::ptr::write_bytes(switch_sp as *mut u8, 0, 280 + 104);

        // Set up __switch callee-saved frame (104 bytes at switch_sp):
        // offset 0 = ra → trap_return_user
        let sw = switch_sp as *mut usize;
        *sw.add(0) = crate::arch::trap::trap_return_user_addr();
        // s0-s11 (offset 8..96) = 0 (already zeroed)

        // Set up TrapContext (280 bytes at trap_ctx_base):
        let ctx = trap_ctx_base as *mut usize;
        // x[2] = kernel_stack_top (sp during kernel trap handling)
        *ctx.add(2) = kernel_stack_top;
        // sstatus at offset 256/8 = 32: SPP=0, SPIE=1
        *ctx.add(32) = 0x20;
        // sepc at offset 264/8 = 33: entry point
        *ctx.add(33) = entry;
        // sscratch at offset 272/8 = 34: user sp
        *ctx.add(34) = user_stack_top;
    }

    // The task's saved SP points to the trap context
    TASK_SPS[tid].store(switch_sp, Ordering::Relaxed);

    // Create TCB
    let mut tcb = TaskControlBlock::new(tid);
    tcb.context = TaskContext::goto(entry, kernel_stack_top);
    sched.tasks[tid] = Some(tcb);
    sched.task_to_process[tid] = process_idx;
    sched.count += 1;

    // If this is the first task being created, assign it to the calling hart
    let h = hartid();
    if HART_CURRENT_TASK[h].load(Ordering::Relaxed) == usize::MAX {
        HART_CURRENT_TASK[h].store(tid, Ordering::Relaxed);
    }

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

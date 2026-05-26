// kernel/src/sched/mod.rs — Round-Robin task scheduler with context switching

pub mod task;

use core::arch::global_asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::mm::pmm;
use crate::sync::spinlock::SpinLock;
use task::{TaskContext, TaskControlBlock, TaskState};

// Embed the context-switch assembly
global_asm!(include_str!("switch.S"));

const MAX_TASKS: usize = 64;
const NUM_TASKS: usize = 3;

// External assembly function: saves current SP to *current_sp, restores SP from *next_sp
unsafe extern "C" {
    fn __switch(current_sp: *mut usize, next_sp: *const usize);
}

// Per-task saved stack pointers, accessible without locking the scheduler.
// Written by __switch (assembly) and during task creation (Rust).
// AtomicUsize is #[repr(transparent)] over usize, so casting to *mut usize is safe.
static TASK_SPS: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(0) }; MAX_TASKS];

struct Scheduler {
    tasks: [Option<TaskControlBlock>; MAX_TASKS],
    current: usize,
    count: usize,
}

static SCHEDULER: SpinLock<Scheduler> = SpinLock::new(Scheduler {
    tasks: [const { None }; MAX_TASKS],
    current: 0,
    count: 0,
});

/// Task entry functions
extern "C" fn task_func_a() {
    unsafe {
        riscv::register::sstatus::set_sie();
    }
    loop {
        crate::console_println!("[Task A] running");
        for _ in 0..100000 {
            core::hint::spin_loop();
        }
    }
}

extern "C" fn task_func_b() {
    unsafe {
        riscv::register::sstatus::set_sie();
    }
    loop {
        crate::console_println!("[Task B] running");
        for _ in 0..100000 {
            core::hint::spin_loop();
        }
    }
}

extern "C" fn task_func_c() {
    unsafe {
        riscv::register::sstatus::set_sie();
    }
    loop {
        crate::console_println!("[Task C] running");
        for _ in 0..100000 {
            core::hint::spin_loop();
        }
    }
}

/// Initialize the scheduler and create kernel tasks
pub fn init() {
    let entries: [usize; NUM_TASKS] = [
        task_func_a as *const () as usize,
        task_func_b as *const () as usize,
        task_func_c as *const () as usize,
    ];

    {
        let mut sched = SCHEDULER.lock();

        for (tid, &entry) in entries.iter().enumerate() {
            // Allocate 4 physical pages for task stack
            let stack_base = match pmm::alloc_frame() {
                Some(f) => f,
                None => {
                    crate::console_println!(
                        "[sched] ERROR: failed to allocate stack for task {}",
                        tid
                    );
                    return;
                }
            };
            // Allocate remaining 3 frames (contiguous, PMM allocates sequentially)
            for _ in 0..3 {
                if pmm::alloc_frame().is_none() {
                    crate::console_println!(
                        "[sched] ERROR: failed to allocate stack for task {}",
                        tid
                    );
                    return;
                }
            }
            let stack_top = stack_base + 4 * pmm::page_size();

            // Push initial callee-saved register frame onto the stack.
            // Layout must match __switch's expectations:
            //   offset 0:  ra  (entry point)
            //   offset 8:  s0  (0)
            //   ...
            //   offset 96: s11 (0)
            // Total: 13 registers × 8 bytes = 104 bytes
            let saved_sp = stack_top - 13 * 8;
            unsafe {
                core::ptr::write(saved_sp as *mut usize, entry); // ra = entry
                for i in 1..13 {
                    core::ptr::write((saved_sp + i * 8) as *mut usize, 0); // s0-s11 = 0
                }
            }

            // Store the saved stack pointer for __switch
            TASK_SPS[tid].store(saved_sp, Ordering::Relaxed);

            // Create the TCB
            let mut tcb = TaskControlBlock::new(tid);
            tcb.context = TaskContext::goto(entry, stack_top);
            sched.tasks[tid] = Some(tcb);
        }

        sched.count = NUM_TASKS;
        sched.current = 0;
        // Mark task 0 as Running initially
        if let Some(ref mut t) = sched.tasks[0] {
            t.state = TaskState::Running;
        }
    }

    crate::console_println!(
        "[sched] Scheduler initialized with {} tasks (context-switching mode)",
        NUM_TASKS
    );
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

/// Mark the current task as exited.
pub fn mark_current_exited() {
    let mut sched = SCHEDULER.lock();
    let current = sched.current;
    if let Some(ref mut t) = sched.tasks[current] {
        t.state = TaskState::Exited;
    }
}

/// Add a user process to the scheduler.
/// `process` is the user process created by Process::from_elf().
/// Returns the task ID or None on failure.
pub fn add_user_process(
    entry: usize,
    user_stack_top: usize,
    kernel_stack_top: usize,
    _page_table_ppn: usize,
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
        // x[0] = 0 (zero)
        // x[2] = kernel_stack_top (will be sp during kernel trap handling)
        *ctx.add(2) = kernel_stack_top;
        // sstatus at offset 256/8 = 32: SPP=0, SPIE=1
        *ctx.add(32) = 0x20; // SPIE bit set
        // sepc at offset 264/8 = 33: entry point
        *ctx.add(33) = entry;
        // sscratch at offset 272/8 = 34: user sp
        *ctx.add(34) = user_stack_top;
    }

    // The task's saved SP points to the trap context
    TASK_SPS[tid].store(trap_ctx_base, Ordering::Relaxed);

    // Create TCB
    let mut tcb = TaskControlBlock::new(tid);
    tcb.context = TaskContext::goto(entry, kernel_stack_top);
    sched.tasks[tid] = Some(tcb);
    sched.count += 1;

    Some(tid)
}

/// Get current process brk (program break for heap).
pub fn current_brk() -> usize {
    // TODO: per-process brk tracking
    crate::process::USER_HEAP_BASE
}

/// Set current process brk.
pub fn set_current_brk(_addr: usize) {
    // TODO: actually allocate pages for heap growth
}

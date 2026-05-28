// kernel/src/sched/mod.rs — Simple Round-Robin scheduler.
//
// Design:
//   - No idle task. Init is NOT in the scheduler.
//   - Children (tid=0+) are created by sys_spawn.
//   - schedule() saves init's sp to INIT_TASK_SP when switching away.
//   - schedule_exit() restores init via __switch back.
//   - When only init exists, schedule() is a no-op.

pub mod task;

use core::arch::global_asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sync::spinlock::SpinLock;
use task::{TaskControlBlock, TaskState};

global_asm!(include_str!("switch.S"));

// Shim for first task entry via __switch
global_asm!(
    ".globl first_task_shim",
    "first_task_shim:",
    "addi sp, sp, 104",   // Skip __switch frame → sp points to TrapContext
    "j trap_return_user", // Restore U-mode context and sret
);

const MAX_TASKS: usize = 64;

unsafe extern "C" {
    fn __switch(current_sp: *mut usize, next_sp: *const usize);
    fn first_task_shim();
}

static TASK_SPS: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(0) }; MAX_TASKS];

/// Saved kernel sp for init. When schedule() switches from init to a child,
/// __switch saves init's sp here. schedule_exit() uses it to switch back.
static INIT_TASK_SP: AtomicUsize = AtomicUsize::new(0);

struct Scheduler {
    tasks: [Option<TaskControlBlock>; MAX_TASKS],
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

pub fn init() {
    crate::console_println!("[sched] Initialized");
}

/// Round-Robin among child tasks. When init is running (current==0),
/// __switch saves init's sp to INIT_TASK_SP then switches to child.
pub fn schedule() {
    let (switch_from_init, current, next) = {
        let mut sched = SCHEDULER.lock();
        if sched.count == 0 {
            return;
        }
        let current = sched.current;

        // Find next Ready child
        let mut next: usize = current;
        for i in 1..sched.count {
            let candidate = (current + i) % sched.count;
            if let Some(ref t) = sched.tasks[candidate] {
                if t.state == TaskState::Ready {
                    next = candidate;
                    break;
                }
            }
        }
        if next == current {
            return;
        }

        // Mark states
        if current != 0 {
            if let Some(ref mut t) = sched.tasks[current] {
                if t.state == TaskState::Running {
                    t.state = TaskState::Ready;
                }
            }
        }
        if let Some(ref mut t) = sched.tasks[next] {
            t.state = TaskState::Running;
        }
        sched.current = next;
        let next_proc_idx = sched.task_to_process[next];
        crate::process::set_current_index(next_proc_idx);
        crate::process::set_current_page_table_root(crate::process::get_page_table_root(
            next_proc_idx,
        ));
        (current == 0, current, next)
    };

    if switch_from_init {
        // Save init's sp to INIT_TASK_SP, switch to child
        let init_sp_ptr = INIT_TASK_SP.as_ptr() as *mut usize;
        let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
        unsafe {
            __switch(init_sp_ptr, nxt_ptr);
        }
    } else {
        let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
        let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
        unsafe {
            __switch(cur_ptr, nxt_ptr);
        }
    }
}

/// Child exits → switch to next child or back to init.
pub fn schedule_exit() {
    let (has_next, current, next) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        if let Some(ref mut t) = sched.tasks[current] {
            t.state = TaskState::Exited;
        }

        let mut next: usize = 0; // 0 means "init"
        for i in 1..sched.count {
            let candidate = (current + i) % sched.count;
            if let Some(ref t) = sched.tasks[candidate] {
                if t.state == TaskState::Ready {
                    next = candidate;
                    break;
                }
            }
        }

        let switch_to_init = next == 0;
        if !switch_to_init {
            if let Some(ref mut t) = sched.tasks[next] {
                t.state = TaskState::Running;
            }
            let next_proc_idx = sched.task_to_process[next];
            crate::process::set_current_index(next_proc_idx);
            crate::process::set_current_page_table_root(crate::process::get_page_table_root(
                next_proc_idx,
            ));
        } else {
            crate::process::set_current_index(0);
            crate::process::set_current_page_table_root(crate::process::get_page_table_root(0));
        }
        sched.current = 0; // Reset to init

        (switch_to_init, current, next)
    };

    if has_next {
        // Switch to another child
        let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
        let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
        unsafe {
            __switch(cur_ptr, nxt_ptr);
        }
    } else {
        // Switch back to init
        let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
        let init_sp_ptr = INIT_TASK_SP.as_ptr() as *const usize;
        unsafe {
            __switch(cur_ptr, init_sp_ptr);
        }
    }
}

pub fn mark_current_exited() {
    let mut sched = SCHEDULER.lock();
    let cur = sched.current;
    if let Some(ref mut t) = sched.tasks[cur] {
        t.state = TaskState::Exited;
    }
}

pub fn schedule_block() {
    let (current, next) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        if let Some(ref mut t) = sched.tasks[current] {
            t.state = TaskState::Blocked;
        }

        let mut next: usize = current;
        for i in 1..sched.count {
            let candidate = (current + i) % sched.count;
            if let Some(ref t) = sched.tasks[candidate] {
                if t.state == TaskState::Ready {
                    next = candidate;
                    break;
                }
            }
        }
        if next == current {
            return;
        }

        if let Some(ref mut t) = sched.tasks[next] {
            t.state = TaskState::Running;
        }
        sched.current = next;
        let next_proc_idx = sched.task_to_process[next];
        crate::process::set_current_index(next_proc_idx);
        crate::process::set_current_page_table_root(crate::process::get_page_table_root(
            next_proc_idx,
        ));
        (current, next)
    };

    let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
    let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
    unsafe {
        __switch(cur_ptr, nxt_ptr);
    }
}

pub fn wake_task(proc_idx: usize) {
    let mut sched = SCHEDULER.lock();
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

pub fn remove_task(proc_idx: usize) {
    let mut sched = SCHEDULER.lock();
    for i in 0..sched.count {
        if sched.task_to_process[i] == proc_idx {
            sched.tasks[i] = None;
            return;
        }
    }
}

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

    let trap_ctx_base = kernel_stack_top - 280;
    let switch_sp = trap_ctx_base - 104;
    unsafe {
        // Write __switch frame + TrapContext (safe: child's kernel stack is fresh)
        core::ptr::write_bytes(switch_sp as *mut u8, 0, 280 + 104);
        let sw = switch_sp as *mut usize;
        *sw.add(0) = first_task_shim as *const () as usize;
        let ctx = trap_ctx_base as *mut usize;
        *ctx.add(2) = kernel_stack_top;
        *ctx.add(32) = 0x20;
        *ctx.add(33) = entry;
        *ctx.add(34) = user_stack_top;
    }
    TASK_SPS[tid].store(switch_sp, Ordering::Relaxed);

    let tcb = TaskControlBlock::new(tid);
    sched.tasks[tid] = Some(tcb);
    sched.task_to_process[tid] = process_idx;
    sched.count += 1;
    Some(tid)
}

pub fn current_task_id() -> usize {
    0
}

pub fn current_brk() -> usize {
    crate::process::current_brk()
}

pub fn set_current_brk(addr: usize) {
    crate::process::set_current_brk(addr);
}

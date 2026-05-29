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

// Shim for first task entry via __switch.
// __switch already pops its own 104-byte frame before `ret`ing here, so sp
// already points at the TrapContext. Jump straight into the U-mode return path,
// which will switch satp (via TrapContext.user_satp) and sret into the task.
global_asm!(
    ".globl first_task_shim",
    "first_task_shim:",
    "j trap_return_user",
);

pub const MAX_TASKS: usize = 64;

/// Sentinel value for `Scheduler::current` meaning "init (the shell) is running".
/// Init is NOT a TaskControlBlock; its saved kernel sp lives in INIT_TASK_SP.
const INIT_SENTINEL: usize = MAX_TASKS;

/// PROCESS_TABLE index of the init process (the shell).
const INIT_PROC_IDX: usize = 0;

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
    current: INIT_SENTINEL, // init is running, no child scheduled yet
    count: 0,
});

pub fn init() {
    crate::console_println!("[sched] Initialized");
}

/// Round-Robin among child tasks. When init is running
/// (current == INIT_SENTINEL), __switch saves init's sp to INIT_TASK_SP then
/// switches to the next Ready child. When a child is running, rotate to the
/// next Ready child after it (if any). Returning to init is handled by
/// schedule_exit() when a child exits, not here.
pub fn schedule() {
    let (switch_from_init, current, next) = {
        let mut sched = SCHEDULER.lock();
        if sched.count == 0 {
            return;
        }
        let current = sched.current;
        let init_running = current == INIT_SENTINEL;
        let count = sched.count;

        // Find the next Ready child. When init is running, scan from slot 0;
        // when a child is running, scan starting after it (round-robin).
        let start = if init_running { 0 } else { current + 1 };
        let mut next = INIT_SENTINEL;
        for i in 0..count {
            let candidate = (start + i) % count;
            if let Some(ref t) = sched.tasks[candidate] {
                if t.state == TaskState::Ready {
                    next = candidate;
                    break;
                }
            }
        }
        if next == INIT_SENTINEL {
            return; // no Ready child to switch to — keep running current
        }

        // Demote the currently-running child back to Ready (init has no TCB).
        if !init_running {
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
        (init_running, current, next)
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

/// Current child exits → switch to another Ready child, or back to init if
/// none remain. Called from sys_exit (current is always a child here).
pub fn schedule_exit() {
    let (switch_to_init, current, next) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        let count = sched.count;
        if current < count {
            if let Some(ref mut t) = sched.tasks[current] {
                t.state = TaskState::Exited;
            }
        }

        // Look for another Ready child, starting after the current one.
        let mut next = INIT_SENTINEL;
        for i in 1..count {
            let candidate = (current + i) % count;
            if let Some(ref t) = sched.tasks[candidate] {
                if t.state == TaskState::Ready {
                    next = candidate;
                    break;
                }
            }
        }

        if next == INIT_SENTINEL {
            // No Ready child left — return control to init (the shell).
            sched.current = INIT_SENTINEL;
            crate::process::set_current_index(INIT_PROC_IDX);
            crate::process::set_current_page_table_root(crate::process::get_page_table_root(
                INIT_PROC_IDX,
            ));
            (true, current, next)
        } else {
            if let Some(ref mut t) = sched.tasks[next] {
                t.state = TaskState::Running;
            }
            sched.current = next;
            let next_proc_idx = sched.task_to_process[next];
            crate::process::set_current_index(next_proc_idx);
            crate::process::set_current_page_table_root(crate::process::get_page_table_root(
                next_proc_idx,
            ));
            (false, current, next)
        }
    };

    let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
    if switch_to_init {
        let init_sp_ptr = INIT_TASK_SP.as_ptr() as *const usize;
        unsafe {
            __switch(cur_ptr, init_sp_ptr);
        }
    } else {
        let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
        unsafe {
            __switch(cur_ptr, nxt_ptr);
        }
    }
}

/// Returns true if the init process (shell) is currently running.
pub fn is_init_running() -> bool {
    let sched = SCHEDULER.lock();
    sched.current == INIT_SENTINEL
}

pub fn mark_current_exited() {
    let mut sched = SCHEDULER.lock();
    let cur = sched.current;
    if cur >= MAX_TASKS {
        return; // init has no TCB
    }
    if let Some(ref mut t) = sched.tasks[cur] {
        t.state = TaskState::Exited;
    }
}

pub fn schedule_block() {
    let (current, next) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        if current >= sched.count {
            return; // init has no TCB / nothing to block
        }
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
    user_satp: usize,
    process_idx: usize,
) -> Option<usize> {
    let mut sched = SCHEDULER.lock();
    // Reuse a freed slot if one exists, otherwise grow into a new slot.
    let tid = match (0..sched.count).find(|&i| sched.tasks[i].is_none()) {
        Some(free) => free,
        None => {
            if sched.count >= MAX_TASKS {
                return None;
            }
            sched.count
        }
    };

    let ctx_size = core::mem::size_of::<crate::arch::trap::TrapContext>();
    let trap_ctx_base = kernel_stack_top - ctx_size;
    let switch_sp = trap_ctx_base - 104;
    unsafe {
        // Write __switch frame + TrapContext (safe: child's kernel stack is fresh)
        core::ptr::write_bytes(switch_sp as *mut u8, 0, ctx_size + 104);
        // __switch frame: ra slot (offset 0) = first_task_shim entry.
        let sw = switch_sp as *mut usize;
        *sw.add(0) = first_task_shim as *const () as usize;
        // TrapContext for the task's first U-mode entry.
        let ctx = trap_ctx_base as *mut usize;
        *ctx.add(2) = user_stack_top; // x[2]: trap_return_user reads this as user sp
        *ctx.add(32) = 0x20; // sstatus: SPP=0 (→U-mode), SPIE=1
        *ctx.add(33) = entry; // sepc: user entry point
        *ctx.add(34) = user_stack_top; // sscratch field (user sp)
        *ctx.add(35) = user_satp; // user_satp: switch to this page table before sret
    }
    TASK_SPS[tid].store(switch_sp, Ordering::Relaxed);

    let tcb = TaskControlBlock::new(tid);
    sched.tasks[tid] = Some(tcb);
    sched.task_to_process[tid] = process_idx;
    if tid >= sched.count {
        sched.count = tid + 1;
    }
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

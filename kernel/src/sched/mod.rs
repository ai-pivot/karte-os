// kernel/src/sched/mod.rs — Round-Robin task scheduler (simplified)

pub mod task;

use task::{TaskControlBlock, TaskState};
use crate::mm::pmm;
use crate::sync::spinlock::SpinLock;

const MAX_TASKS: usize = 64;
#[allow(dead_code)]
const TASK_STACK_SIZE: usize = 32 * 1024; // 32KB per task stack

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

pub fn init() {
    // For now, the scheduler is initialized but tasks are not running.
    // Timer interrupts will drive periodic output.
    // Full context-switching will be added in future versions.
    crate::console_println!("[sched] Scheduler initialized (timer-driven mode)");
}

/// Called from timer interrupt handler
pub fn schedule() {
    let mut sched = SCHEDULER.lock();
    if sched.count == 0 {
        return;
    }

    let current = sched.current;
    let next = (current + 1) % sched.count;
    sched.current = next;

    if let Some(ref mut t) = sched.tasks[current] {
        t.state = TaskState::Ready;
    }
    if let Some(ref mut t) = sched.tasks[next] {
        t.state = TaskState::Running;
    }
}

/// Get current task's ID
pub fn current_task_id() -> usize {
    SCHEDULER.lock().current
}

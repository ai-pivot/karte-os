// kernel/src/sched/task.rs — Task data structures for context-switching scheduler

/// Task state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Exited,
}

/// Task context for context switching (callee-saved registers).
/// This struct is used for initial task setup. At runtime, the actual
/// register state is saved on the task's kernel stack by __switch.
#[repr(C)]
pub struct TaskContext {
    pub ra: usize,
    pub sp: usize,
    pub s: [usize; 12], // s0-s11
}

impl TaskContext {
    pub fn new() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s: [0; 12],
        }
    }

    /// Create a new task context that will start executing at `entry` with given stack
    pub fn goto(entry: usize, kstack_top: usize) -> Self {
        Self {
            ra: entry,
            sp: kstack_top,
            s: [0; 12],
        }
    }
}

/// Task control block
pub struct TaskControlBlock {
    pub context: TaskContext,
    pub tid: usize,
    pub state: TaskState,
}

impl TaskControlBlock {
    pub fn new(tid: usize) -> Self {
        Self {
            context: TaskContext::new(),
            tid,
            state: TaskState::Ready,
        }
    }
}

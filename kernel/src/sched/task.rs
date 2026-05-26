// kernel/src/sched/task.rs — Task data structures

/// Task state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Exited,
}

/// Task control block
#[repr(C)]
pub struct TaskControlBlock {
    /// Saved kernel stack pointer (for context switch)
    pub kstack_ptr: usize,
    /// Task ID
    pub tid: usize,
    /// Task state
    pub state: TaskState,
    /// Entry point
    pub entry: usize,
}

impl TaskControlBlock {
    pub fn new(tid: usize, entry: usize, stack_top: usize) -> Self {
        Self {
            kstack_ptr: stack_top,
            tid,
            state: TaskState::Ready,
            entry,
        }
    }
}

/// Task context for context switching (callee-saved registers)
#[repr(C)]
pub struct TaskContext {
    pub ra: usize, // Return address
    pub sp: usize, // Stack pointer
    pub s0: usize, // s0/fp
    pub s1: usize,
    pub s2: usize,
    pub s3: usize,
    pub s4: usize,
    pub s5: usize,
    pub s6: usize,
    pub s7: usize,
    pub s8: usize,
    pub s9: usize,
    pub s10: usize,
    pub s11: usize,
}

impl TaskContext {
    pub fn new() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s0: 0,
            s1: 0,
            s2: 0,
            s3: 0,
            s4: 0,
            s5: 0,
            s6: 0,
            s7: 0,
            s8: 0,
            s9: 0,
            s10: 0,
            s11: 0,
        }
    }

    pub fn set_entry(&mut self, entry: usize) {
        self.ra = entry;
    }

    pub fn set_stack(&mut self, sp: usize) {
        self.sp = sp;
    }
}

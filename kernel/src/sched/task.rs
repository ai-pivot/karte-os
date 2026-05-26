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

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── Task Tests ──");

    // Test 1: TaskContext new is zeroed
    crate::test::run_test("task_context_new_zeroed", || {
        let ctx = TaskContext::new();
        ctx.ra == 0 && ctx.sp == 0 && ctx.s.iter().all(|&v| v == 0)
    });

    // Test 2: TaskContext goto sets ra and sp
    crate::test::run_test("task_context_goto", || {
        let ctx = TaskContext::goto(0x8020_0000, 0x8030_0000);
        ctx.ra == 0x8020_0000 && ctx.sp == 0x8030_0000
    });

    // Test 3: TaskControlBlock new
    crate::test::run_test("task_tcb_new", || {
        let tcb = TaskControlBlock::new(42);
        tcb.tid == 42 && tcb.state == TaskState::Ready
    });

    // Test 4: Task state transitions
    crate::test::run_test("task_state_transitions", || {
        let mut tcb = TaskControlBlock::new(1);
        if tcb.state != TaskState::Ready {
            return false;
        }
        tcb.state = TaskState::Running;
        if tcb.state != TaskState::Running {
            return false;
        }
        tcb.state = TaskState::Blocked;
        if tcb.state != TaskState::Blocked {
            return false;
        }
        tcb.state = TaskState::Exited;
        tcb.state == TaskState::Exited
    });

    // Test 5: Multiple TCBs with unique IDs
    crate::test::run_test("task_unique_tcb_ids", || {
        let t1 = TaskControlBlock::new(0);
        let t2 = TaskControlBlock::new(1);
        let t3 = TaskControlBlock::new(2);
        t1.tid != t2.tid && t2.tid != t3.tid && t1.tid != t3.tid
    });

    // Test 6: TaskContext goto preserves s registers as zero
    crate::test::run_test("task_context_goto_s_zeroed", || {
        let ctx = TaskContext::goto(0x1000, 0x2000);
        ctx.s.iter().all(|&v| v == 0)
    });
}

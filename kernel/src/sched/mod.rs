// kernel/src/sched/mod.rs — unified task scheduler.
//
// Design:
//   - The scheduler knows tasks, not "init". Shell is just the first User task.
//   - A kernel Idle task is the only fallback when no user task is runnable.
//   - Every User task has a valid saved_sp before it can be scheduled.

pub mod class;
pub mod ready_queue;
pub mod task;

use core::arch::global_asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sync::spinlock::SpinLock;
use alloc::boxed::Box;
use alloc::vec::Vec;
use class::SchedClass;
use ready_queue::ReadyQueue;
use task::{TaskControlBlock, TaskState};

#[cfg(target_arch = "riscv64")]
global_asm!(include_str!("../arch/riscv64/switch.S"));

#[cfg(target_arch = "riscv64")]
global_asm!(
    ".globl first_task_shim",
    "first_task_shim:",
    "j trap_return_user",
);

#[cfg(target_arch = "x86_64")]
unsafe extern "C" {
    fn __switch(current_sp: *mut usize, next_sp: *const usize);
}

#[cfg(target_arch = "riscv64")]
unsafe extern "C" {
    fn __switch(current_sp: *mut usize, next_sp: *const usize);
    fn first_task_shim();
}

#[cfg(target_arch = "x86_64")]
unsafe extern "C" {
    fn trap_return_user(ctx: *mut crate::arch::trap::TrapContext) -> !;
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn first_task_shim() -> ! {
    unsafe {
        core::arch::naked_asm!(
            "mov rdi, rsp",
            "jmp {handler}",
            handler = sym trap_return_user,
        );
    }
}

/// Initial capacity of the dynamic task table; the Vec grows on demand.
/// (P1.1: replaced the fixed MAX_TASKS=64 slot array — a 200-task stress
/// workload must schedule fine.)
pub const INITIAL_TASK_CAPACITY: usize = 256;

const IDLE_SLOT: usize = 0;
const NO_SLOT: usize = usize::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskKind {
    Empty,
    Idle,
    User { proc_idx: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedError {
    NoFreeSlot,
    InvalidTask,
}

#[derive(Clone, Copy)]
pub struct UserTaskInit {
    pub entry: usize,
    pub user_stack_top: usize,
    pub kernel_stack_top: usize,
    /// RISC-V: full SATP value. x86_64: CR3 physical address.
    pub user_page_table: usize,
}

#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy)]
pub struct CloneTaskInit<'a> {
    pub parent_ctx: &'a crate::arch::trap::TrapContext,
    pub new_user_sp: usize,
    pub kernel_stack_top: usize,
    pub user_cr3: usize,
    pub tls: usize,
}

static PROC_TO_SLOT: SpinLock<Vec<AtomicUsize>> = SpinLock::new(Vec::new());

/// proc_idx → slot. Lock discipline: this lock is NEVER held while taking
/// (or while holding) the SCHEDULER lock — callers either load the slot
/// first and lock the scheduler afterwards, or set the mapping after
/// releasing the scheduler lock.
fn proc_slot_get(proc_idx: usize) -> usize {
    let map = PROC_TO_SLOT.lock();
    map.get(proc_idx)
        .map(|a| a.load(Ordering::Relaxed))
        .unwrap_or(NO_SLOT)
}

fn proc_slot_set(proc_idx: usize, slot: usize) {
    let mut map = PROC_TO_SLOT.lock();
    if proc_idx >= map.len() {
        map.resize_with(proc_idx + 1, || AtomicUsize::new(NO_SLOT));
    }
    map[proc_idx].store(slot, Ordering::Relaxed);
}

/// Currently running scheduler slot. Slot 0 is the typed Idle task, not init.
pub static CURRENT_RUNNING: AtomicUsize = AtomicUsize::new(IDLE_SLOT);

static LAST_SCHEDULED: AtomicUsize = AtomicUsize::new(IDLE_SLOT);

/// Per-task scheduling data. Lives in a heap box whose address is stable for
/// the whole lifetime of its slot: `__switch` receives a raw pointer to `sp`,
/// so boxes are only dropped when their slot is REUSED by a new task — never
/// while a context switch may still reference them (exit marks the node and
/// recycles the slot, it does not take the box).
struct TaskNode {
    kind: TaskKind,
    state: TaskState,
    class: SchedClass,
    /// Saved __switch stack pointer (was the TASK_SPS static array).
    sp: AtomicUsize,
    initial_sp: AtomicUsize,
    /// Remaining scheduler ticks before this task must requeue (RR/FIFO).
    quantum_left: usize,
    #[cfg(target_arch = "x86_64")]
    kstack: core::sync::atomic::AtomicU64,
    #[cfg(target_arch = "x86_64")]
    fs_base: core::sync::atomic::AtomicU64,
}

impl TaskNode {
    fn new_idle() -> Self {
        Self {
            kind: TaskKind::Idle,
            state: TaskState::Running,
            class: SchedClass::Normal,
            sp: AtomicUsize::new(0),
            initial_sp: AtomicUsize::new(0),
            quantum_left: usize::MAX,
            #[cfg(target_arch = "x86_64")]
            kstack: core::sync::atomic::AtomicU64::new(0),
            #[cfg(target_arch = "x86_64")]
            fs_base: core::sync::atomic::AtomicU64::new(0),
        }
    }

    fn new_user(proc_idx: usize, class: SchedClass, initial_sp: usize) -> Self {
        Self {
            kind: TaskKind::User { proc_idx },
            state: TaskState::Ready,
            class,
            sp: AtomicUsize::new(initial_sp),
            initial_sp: AtomicUsize::new(initial_sp),
            quantum_left: class.quantum(),
            #[cfg(target_arch = "x86_64")]
            kstack: core::sync::atomic::AtomicU64::new(0),
            #[cfg(target_arch = "x86_64")]
            fs_base: core::sync::atomic::AtomicU64::new(0),
        }
    }
}

struct Scheduler {
    nodes: Vec<Option<Box<TaskNode>>>,
    /// Slots whose task exited; the box is reused (dropped) on next alloc.
    free: Vec<usize>,
    /// 32-level priority-bitmap ready queue (P1.1).
    ready: ReadyQueue,
    current: usize,
    high_water: usize,
}

static SCHEDULER: SpinLock<Scheduler> = SpinLock::new(Scheduler {
    nodes: Vec::new(),
    free: Vec::new(),
    ready: ReadyQueue::new(),
    current: IDLE_SLOT,
    high_water: 1,
});

pub fn init() {
    let mut sched = SCHEDULER.lock();
    sched
        .nodes
        .push(Some(alloc::boxed::Box::new(TaskNode::new_idle())));
    sched.current = IDLE_SLOT;
    sched.high_water = 1;
    CURRENT_RUNNING.store(IDLE_SLOT, Ordering::Relaxed);
}

pub fn current_running_slot() -> usize {
    CURRENT_RUNNING.load(Ordering::Relaxed)
}

pub fn current_sched_slot() -> usize {
    current_running_slot()
}

pub fn current_user_proc() -> Option<usize> {
    let sched = SCHEDULER.lock();
    node_ref(&sched, sched.current).map(|n| match n.kind {
        TaskKind::User { proc_idx } => Some(proc_idx),
        _ => None,
    })?
}

#[cfg(target_arch = "x86_64")]
pub fn current_kernel_stack() -> Option<u64> {
    let current = CURRENT_RUNNING.load(Ordering::Relaxed);
    let sched = SCHEDULER.lock();
    let ksp = node_ref(&sched, current).map(|n| n.kstack.load(Ordering::Relaxed));
    match ksp {
        Some(ksp) if ksp != 0 => Some(ksp),
        _ => None,
    }
}

#[cfg(target_arch = "x86_64")]
static PENDING_RSP0: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// O(1) pick-next via the priority-bitmap ready queue (P1.1). The running
/// task is never in the queue, so the returned slot (if any) is a different
/// task — except the single-task case where `schedule` re-enqueues current.
fn find_next_ready_user(sched: &mut Scheduler, _current: usize) -> Option<usize> {
    sched.ready.pop_next()
}

fn node_ref(sched: &Scheduler, slot: usize) -> Option<&TaskNode> {
    sched.nodes.get(slot).and_then(|n| n.as_deref())
}

fn node_mut(sched: &mut Scheduler, slot: usize) -> Option<&mut TaskNode> {
    sched.nodes.get_mut(slot).and_then(|n| n.as_deref_mut())
}

fn set_current_process_for_slot(slot: usize) {
    let kind = {
        let sched = SCHEDULER.lock();
        node_ref(&sched, slot).map(|n| n.kind)
    };
    match kind {
        Some(TaskKind::User { proc_idx }) => {
            crate::process::set_current_index(proc_idx);
            crate::process::set_current_page_table_root(crate::process::get_page_table_root(
                proc_idx,
            ));
        }
        _ => {
            crate::process::set_current_page_table_root(0);
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn save_fs_base(slot: usize) {
    // Read the CURRENT hardware FS_BASE from MSR and save it.
    let fs_base = unsafe { crate::arch::idt::rdmsr(0xC0000100) };
    let mut sched = SCHEDULER.lock();
    if let Some(n) = node_mut(&mut sched, slot) {
        n.fs_base.store(fs_base, Ordering::Relaxed);
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn save_fs_base(_slot: usize) {}

#[cfg(target_arch = "x86_64")]
fn restore_task_arch_state(slot: usize) {
    let (kernel_sp, fs_base) = {
        let sched = SCHEDULER.lock();
        match node_ref(&sched, slot) {
            Some(n) => (
                n.kstack.load(Ordering::Relaxed),
                n.fs_base.load(Ordering::Relaxed),
            ),
            None => return,
        }
    };
    if kernel_sp != 0 {
        crate::arch::idt::set_syscall_ksp(kernel_sp);
        unsafe {
            crate::arch::gdt::set_kernel_rsp0_for_cpu(0, kernel_sp);
        }
    }
    // Do not switch to a task's user CR3 here. __switch() resumes arbitrary
    // kernel continuations (syscall handlers, timer handlers, idle paths), not
    // necessarily an immediate user return. User CR3 is installed only at the
    // explicit user-return paths (iretq/trap_return_user/syscall return).
    unsafe { crate::arch::idt::wrmsr(0xC0000100, fs_base) };
}

#[cfg(not(target_arch = "x86_64"))]
fn restore_task_arch_state(_slot: usize) {}

/// Test-only access to restore_task_arch_state for verifying arch-state restore.
#[cfg(all(target_arch = "x86_64", feature = "test_mode"))]
pub fn restore_task_arch_state_for_test(slot: usize) {
    restore_task_arch_state(slot);
}

/// Get the typed `UserReturnState` for the currently scheduled task.
/// Used by syscall return paths to restore all per-task state.
#[cfg(target_arch = "x86_64")]
pub fn current_user_return_state() -> crate::arch::user_return::UserReturnState {
    let slot = current_sched_slot();
    user_return_state_for_slot(slot)
}

/// Get the typed `UserReturnState` for a specific scheduler slot.
#[cfg(target_arch = "x86_64")]
pub fn user_return_state_for_slot(slot: usize) -> crate::arch::user_return::UserReturnState {
    use crate::arch::user_return::*;

    let (fs_base_raw, kernel_sp) = {
        let sched = SCHEDULER.lock();
        match node_ref(&sched, slot) {
            Some(n) => (
                n.fs_base.load(Ordering::Relaxed),
                n.kstack.load(Ordering::Relaxed),
            ),
            None => (0, 0),
        }
    };

    // user_cr3 is not tracked per-slot in the scheduler (it's in Process).
    // The caller should set user_cr3 separately if needed.
    let kernel_rsp0 = if kernel_sp != 0 {
        Some(KernelRsp0::new(kernel_sp))
    } else {
        None
    };

    UserReturnState {
        user_cr3: None, // Set by caller from Process page_table_root
        kernel_rsp0,
        fs_base: FsBase::new(fs_base_raw),
    }
}

fn switch_to(current: usize, next: usize) {
    save_fs_base(current);
    CURRENT_RUNNING.store(next, Ordering::Relaxed);
    set_current_process_for_slot(next);

    #[cfg(target_arch = "x86_64")]
    {
        let next_fs_base = {
            let sched = SCHEDULER.lock();
            node_ref(&sched, next)
                .map(|n| n.fs_base.load(Ordering::Relaxed))
                .unwrap_or(0)
        };
        crate::arch::trap::PENDING_FS_BASE.store(next_fs_base, Ordering::Relaxed);

        let effective_kcr3 = {
            let idt_val = crate::arch::idt::get_kernel_cr3_phys() as u64;
            if idt_val != 0 {
                idt_val
            } else {
                crate::mm::vmm::kernel_cr3()
            }
        };
        if effective_kcr3 != 0 {
            unsafe {
                core::arch::asm!("mov cr3, {}", in(reg) effective_kcr3);
            }
        }
    }

    // Take stable raw pointers to the saved-SP cells under the lock, then
    // release it before __switch. Boxes are never dropped while a slot exists
    // (they are reset-in-place on reuse), so the pointers stay valid across
    // the switch, including the exit path where current is already marked.
    let (cur_ptr, nxt_ptr) = {
        let mut sched = SCHEDULER.lock();
        let cur = node_mut(&mut sched, current).map(|n| &n.sp as *const AtomicUsize as *mut usize);
        let nxt = node_mut(&mut sched, next).map(|n| &n.sp as *const AtomicUsize as *const usize);
        (cur, nxt)
    };
    let (cur_ptr, nxt_ptr) = match (cur_ptr, nxt_ptr) {
        (Some(c), Some(n)) => (c, n),
        _ => return, // slot vanished (should not happen); refuse to switch
    };
    unsafe {
        __switch(cur_ptr, nxt_ptr);
    }

    let resumed = CURRENT_RUNNING.load(Ordering::Relaxed);
    restore_task_arch_state(resumed);
}

pub fn schedule() {
    let mut sched_guard = SCHEDULER.lock();
    let current = sched_guard.current;

    // Quantum gate: timer-driven rescheduling happens only when the running
    // task's slice is exhausted. Blocking/exit paths bypass this via
    // schedule_block/schedule_exit.
    if let Some(n) = node_mut(&mut sched_guard, current) {
        if matches!(n.kind, TaskKind::User { .. })
            && n.state == TaskState::Running
            && n.quantum_left > 0
        {
            n.quantum_left -= 1;
            return;
        }
    }

    // Requeue current if still runnable (Running -> Ready), then O(1)-pick
    // the highest-priority ready task.
    let next = {
        if let Some(n) = node_mut(&mut sched_guard, current) {
            if matches!(n.kind, TaskKind::User { .. }) && n.state == TaskState::Running {
                n.state = TaskState::Ready;
                n.quantum_left = n.class.quantum();
                let prio = n.class.priority();
                sched_guard.ready.push(current, prio);
            }
        }
        match find_next_ready_user(&mut sched_guard, current) {
            Some(slot) => slot,
            // No other ready task: keep running current (single-task case).
            None => {
                if let Some(n) = node_mut(&mut sched_guard, current) {
                    if n.state == TaskState::Ready {
                        n.state = TaskState::Running;
                    }
                }
                LAST_SCHEDULED.store(current, Ordering::Relaxed);
                return;
            }
        }
    };

    if let Some(n) = node_mut(&mut sched_guard, next) {
        n.state = TaskState::Running;
    }
    sched_guard.current = next;
    LAST_SCHEDULED.store(next, Ordering::Relaxed);
    drop(sched_guard);

    switch_to(current, next);
}

pub fn schedule_block() {
    let mut sched_guard = SCHEDULER.lock();
    let current = sched_guard.current;
    let is_user = node_ref(&sched_guard, current)
        .map(|n| matches!(n.kind, TaskKind::User { .. }))
        .unwrap_or(false);
    if !is_user {
        return;
    }
    // Blocked tasks leave the ready queue (invariant: Ready ⇔ queued).
    if let Some(n) = node_mut(&mut sched_guard, current) {
        n.state = TaskState::Blocked;
        sched_guard.ready.remove(current);
    }

    let next = find_next_ready_user(&mut sched_guard, current).unwrap_or(IDLE_SLOT);
    if let Some(n) = node_mut(&mut sched_guard, next) {
        n.state = TaskState::Running;
    }
    sched_guard.current = next;
    LAST_SCHEDULED.store(next, Ordering::Relaxed);
    drop(sched_guard);

    switch_to(current, next);
}

pub fn schedule_exit() {
    remove_sleep(CURRENT_RUNNING.load(Ordering::Relaxed));
    let (proc_idx, current, next) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        let proc_idx = node_ref(&sched, current).and_then(|n| match n.kind {
            TaskKind::User { proc_idx } => Some(proc_idx),
            _ => None,
        });
        // Mark exited and recycle the SLOT; the box itself stays in place
        // (reset on reuse) so its address — captured by switch_to below —
        // remains valid across the switch.
        if let Some(n) = node_mut(&mut sched, current) {
            n.state = TaskState::Exited;
            n.kind = TaskKind::Empty;
            sched.ready.remove(current);
            sched.free.push(current);
        }

        let next = find_next_ready_user(&mut sched, current).unwrap_or(IDLE_SLOT);
        if let Some(n) = node_mut(&mut sched, next) {
            n.state = TaskState::Running;
        }
        sched.current = next;
        LAST_SCHEDULED.store(next, Ordering::Relaxed);
        (proc_idx, current, next)
    };
    // Mapping cleanup outside the scheduler lock (PROC_TO_SLOT lock is never
    // held together with SCHEDULER).
    if let Some(p) = proc_idx {
        proc_slot_set(p, NO_SLOT);
    }

    switch_to(current, next);
}

/// Kept for syscall shutdown policy. PID 1 is the init process; it is not
/// identified by a scheduler slot.
pub fn is_init_running() -> bool {
    crate::process::current_pid() == 1
}

pub fn mark_current_exited() {
    remove_sleep(CURRENT_RUNNING.load(Ordering::Relaxed));
    let mut sched = SCHEDULER.lock();
    let cur = sched.current;
    if let Some(n) = node_mut(&mut sched, cur) {
        n.state = TaskState::Exited;
    }
}

pub fn mark_task_exited_by_proc(proc_idx: usize) {
    let slot = proc_slot_get(proc_idx);
    if slot == NO_SLOT {
        return;
    }
    remove_sleep(slot);
    let mut sched = SCHEDULER.lock();
    if let Some(n) = node_mut(&mut sched, slot) {
        n.state = TaskState::Exited;
        n.kind = TaskKind::Empty;
        sched.ready.remove(slot);
        sched.free.push(slot);
    }
    drop(sched);
    proc_slot_set(proc_idx, NO_SLOT);
}

pub fn wake_task(proc_idx: usize) -> bool {
    let slot = proc_slot_get(proc_idx);
    if slot == NO_SLOT {
        return false;
    }
    remove_sleep(slot);
    let mut sched = SCHEDULER.lock();
    if let Some(n) = node_mut(&mut sched, slot) {
        if n.state == TaskState::Blocked {
            n.state = TaskState::Ready;
            let prio = n.class.priority();
            sched.ready.push(slot, prio);
            return true;
        }
    }
    false
}

/// Sleep queue: dynamic (P1.1 — the old fixed 32-entry array could not hold a
/// 200-task stress workload). Entries: (slot, wake_tick).
static SLEEPQ: SpinLock<Vec<(usize, u64)>> = SpinLock::new(Vec::new());

fn remove_sleep(slot: usize) {
    let mut q = SLEEPQ.lock();
    q.retain(|&(s, _)| s != slot);
}

fn queue_sleep(slot: usize, wake_tick: u64) -> bool {
    let mut q = SLEEPQ.lock();
    for entry in q.iter_mut() {
        if entry.0 == slot {
            entry.1 = wake_tick;
            return true;
        }
    }
    q.push((slot, wake_tick));
    true
}

pub fn sleep_until(wake_tick: u64) {
    let now = crate::arch::platform::uptime_ms();
    if wake_tick <= now {
        return;
    }
    let slot = CURRENT_RUNNING.load(Ordering::Relaxed);
    if !is_slot_active(slot) {
        while crate::arch::platform::uptime_ms() < wake_tick {
            core::hint::spin_loop();
        }
        return;
    }
    if queue_sleep(slot, wake_tick) {
        schedule_block();
    } else {
        while crate::arch::platform::uptime_ms() < wake_tick {
            core::hint::spin_loop();
        }
    }
}

/// Tickless idle statistics (P3.4): full 10ms ticks vs merged (deep-wfi)
/// ticks where the next timer was pushed out to the nearest sleep deadline.
pub static TICKLESS_FULL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static TICKLESS_MERGED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Nearest pending sleep deadline in uptime_ms, if any.
pub fn next_sleep_deadline() -> Option<u64> {
    let q = match SLEEPQ.try_lock() {
        Some(guard) => guard,
        None => return None,
    };
    q.iter().map(|e| e.1).min()
}

/// True when no user task is currently runnable (deep idle candidate).
pub fn no_ready_tasks() -> bool {
    let sched = match SCHEDULER.try_lock() {
        Some(guard) => guard,
        None => return false,
    };
    sched.ready.is_empty()
}

pub fn tick_sleep_queue() {
    let now = crate::arch::platform::uptime_ms();
    let mut to_wake: Vec<usize> = Vec::new();
    {
        let mut q = match SLEEPQ.try_lock() {
            Some(guard) => guard,
            None => return,
        };
        // Drain every due entry (queue is now unbounded, so no truncation).
        let mut i = 0;
        while i < q.len() {
            if now >= q[i].1 {
                to_wake.push(q.remove(i).0);
            } else {
                i += 1;
            }
        }
    }

    if to_wake.is_empty() {
        return;
    }
    let mut sched = match SCHEDULER.try_lock() {
        Some(guard) => guard,
        None => return,
    };
    for slot in to_wake {
        if let Some(n) = node_mut(&mut sched, slot) {
            if n.state == TaskState::Blocked {
                n.state = TaskState::Ready;
                let prio = n.class.priority();
                sched.ready.push(slot, prio);
            }
        }
    }
}

#[cfg(target_arch = "riscv64")]
fn build_initial_stack(init: UserTaskInit) -> usize {
    let ctx_size = core::mem::size_of::<crate::arch::trap::TrapContext>();
    let trap_ctx_base = init.kernel_stack_top - ctx_size;
    let switch_sp = trap_ctx_base - 104;
    unsafe {
        core::ptr::write_bytes(switch_sp as *mut u8, 0, ctx_size + 104);
        let sw = switch_sp as *mut usize;
        *sw.add(0) = first_task_shim as *const () as usize;
        let ctx = trap_ctx_base as *mut usize;
        // trap_return_user restores user sp from the x[2] slot (offset 16),
        // NOT from sscratch — first U-mode entry must follow the same
        // convention as trap_handler returns.
        *ctx.add(2) = init.user_stack_top;
        *ctx.add(32) = 0x20;
        *ctx.add(33) = init.entry;
        *ctx.add(34) = init.kernel_stack_top;
        *ctx.add(35) = init.user_page_table;
    }
    switch_sp
}

#[cfg(target_arch = "x86_64")]
fn build_initial_stack(init: UserTaskInit) -> usize {
    let ctx_size = core::mem::size_of::<crate::arch::trap::TrapContext>();
    let switch_frame_size: usize = 8 * 8 + 512;
    let switch_sp = (init.kernel_stack_top - ctx_size - switch_frame_size) & !0xF;
    let trap_ctx_base = switch_sp + switch_frame_size;
    unsafe {
        core::ptr::write_bytes(switch_sp as *mut u8, 0, ctx_size + switch_frame_size);
        let mxcsr_ptr = (switch_sp as *mut u8).add(24) as *mut u32;
        *mxcsr_ptr = 0x1F80;
        let sw = switch_sp as *mut usize;
        *sw.add(512 / 8) = switch_sp + 520; // orig_rsp for __switch pop sequence
        *sw.add(568 / 8) = first_task_shim as *const () as usize;

        let mut ctx = crate::arch::trap::TrapContext::new_for_user(
            init.entry,
            init.user_stack_top,
            init.kernel_stack_top,
        );
        ctx.user_cr3 = init.user_page_table as u64;
        ctx.trap_from_user = 1;
        core::ptr::write(trap_ctx_base as *mut crate::arch::trap::TrapContext, ctx);
    }
    switch_sp
}

#[cfg(target_arch = "x86_64")]
fn build_clone_stack(init: CloneTaskInit<'_>) -> usize {
    let ctx_size = core::mem::size_of::<crate::arch::trap::TrapContext>();
    let switch_frame_size: usize = 8 * 8 + 512;
    let switch_sp = (init.kernel_stack_top - ctx_size - switch_frame_size) & !0xF;
    let trap_ctx_base = switch_sp + switch_frame_size;
    unsafe {
        core::ptr::write_bytes(switch_sp as *mut u8, 0, ctx_size + switch_frame_size);
        let mxcsr_ptr = (switch_sp as *mut u8).add(24) as *mut u32;
        *mxcsr_ptr = 0x1F80;
        let sw = switch_sp as *mut usize;
        *sw.add(512 / 8) = switch_sp + 520; // orig_rsp for __switch pop sequence
        *sw.add(520 / 8) = init.tls;
        *sw.add(568 / 8) = first_task_shim as *const () as usize;

        let mut ctx = init.parent_ctx.clone();
        ctx.rax = 0;
        ctx.rsp = init.new_user_sp as u64;
        ctx.kernel_sp = init.kernel_stack_top as u64;
        ctx.user_cr3 = init.user_cr3 as u64;
        ctx.trap_from_user = 1;
        core::ptr::write(trap_ctx_base as *mut crate::arch::trap::TrapContext, ctx);
    }
    switch_sp
}

fn allocate_user_slot(
    proc_idx: usize,
    kernel_stack_top: usize,
    initial_sp: usize,
) -> Result<usize, SchedError> {
    let mut sched = SCHEDULER.lock();
    // Reuse an exited slot (box reset in place — addresses stay stable for
    // __switch) or grow the table. No fixed cap: the Vec grows on demand.
    let slot = match sched.free.pop() {
        Some(s) => s,
        None => {
            sched
                .nodes
                .push(Some(alloc::boxed::Box::new(TaskNode::new_idle())));
            sched.nodes.len() - 1
        }
    };
    let prio = {
        let n = node_mut(&mut sched, slot).ok_or(SchedError::NoFreeSlot)?;
        *n = TaskNode::new_user(proc_idx, SchedClass::Normal, initial_sp);
        #[cfg(target_arch = "x86_64")]
        n.kstack.store(kernel_stack_top as u64, Ordering::Relaxed);
        n.class.priority()
    };
    if slot >= sched.high_water {
        sched.high_water = slot + 1;
    }
    sched.ready.push(slot, prio);
    drop(sched);
    // Mapping update outside the scheduler lock (lock discipline).
    proc_slot_set(proc_idx, slot);
    Ok(slot)
}

pub fn spawn_user_task(proc_idx: usize, init: UserTaskInit) -> Result<usize, SchedError> {
    let initial_sp = build_initial_stack(init);
    allocate_user_slot(proc_idx, init.kernel_stack_top, initial_sp)
}

/// Fork entry: build the child's kernel stack from the PARENT's saved
/// TrapContext so the child resumes right after its fork() ecall with a0=0
/// (POSIX fork semantics). P1.2: replaces the old "restart at ELF entry"
/// behavior.
///
/// `parent_kernel_stack_top` — parent's kernel stack top; its TrapContext
/// sits at `top - size_of::<TrapContext>()` (pushed by trap_entry.S).
/// `user_satp` — the CHILD's page-table root register value.
#[cfg(target_arch = "riscv64")]
pub fn spawn_forked_task(
    proc_idx: usize,
    kernel_stack_top: usize,
    parent_kernel_stack_top: usize,
    user_satp: usize,
) -> Result<usize, SchedError> {
    let ctx_size = core::mem::size_of::<crate::arch::trap::TrapContext>();
    let trap_ctx_base = kernel_stack_top - ctx_size;
    let switch_sp = trap_ctx_base - 104;
    let parent_ctx = parent_kernel_stack_top - ctx_size;
    unsafe {
        core::ptr::write_bytes(switch_sp as *mut u8, 0, ctx_size + 104);
        // Copy the parent's saved syscall frame wholesale...
        core::ptr::copy_nonoverlapping(parent_ctx as *const u8, trap_ctx_base as *mut u8, ctx_size);
        let ctx = trap_ctx_base as *mut usize;
        // ...then adjust for the child: a0=0, sepc past the ecall, own
        // kernel stack and page table. (x[2] user sp and callee registers
        // are inherited verbatim from the copy.)
        *ctx.add(10) = 0; // x[10] = a0 = 0 (child fork return)
        *ctx.add(33) += 4; // sepc: skip the ecall instruction
        *ctx.add(34) = kernel_stack_top; // sscratch slot = child kernel stack
        *ctx.add(35) = user_satp; // child page table
        let sw = switch_sp as *mut usize;
        *sw.add(0) = first_task_shim as *const () as usize;
    }
    allocate_user_slot(proc_idx, kernel_stack_top, switch_sp)
}

/// x86_64 fork entry: same POSIX semantics, built from the parent's saved
/// int-0x80 TrapContext.
#[cfg(target_arch = "x86_64")]
pub fn spawn_forked_task(
    proc_idx: usize,
    kernel_stack_top: usize,
    parent_kernel_stack_top: usize,
    user_cr3: usize,
) -> Result<usize, SchedError> {
    use crate::arch::trap::TrapContext;
    let ctx_size = core::mem::size_of::<TrapContext>();
    let switch_frame_size: usize = 8 * 8 + 512;
    let switch_sp = (kernel_stack_top - ctx_size - switch_frame_size) & !0xF;
    let trap_ctx_base = switch_sp + switch_frame_size;
    let parent_ctx = parent_kernel_stack_top - ctx_size;
    unsafe {
        core::ptr::write_bytes(switch_sp as *mut u8, 0, ctx_size + switch_frame_size);
        let mxcsr_ptr = (switch_sp as *mut u8).add(24) as *mut u32;
        *mxcsr_ptr = 0x1F80;
        let sw = switch_sp as *mut usize;
        *sw.add(512 / 8) = switch_sp + 520; // orig_rsp for __switch pop sequence
        *sw.add(568 / 8) = first_task_shim as *const () as usize;

        // Copy the parent's saved frame wholesale, then adjust: rax=0 (child
        // return), own kernel stack, own user CR3. RIP already points past
        // the int 0x80 (the ISR saved the return address).
        core::ptr::copy_nonoverlapping(parent_ctx as *const u8, trap_ctx_base as *mut u8, ctx_size);
        let ctx = trap_ctx_base as *mut TrapContext;
        (*ctx).rax = 0;
        (*ctx).kernel_sp = kernel_stack_top as u64;
        (*ctx).user_cr3 = user_cr3 as u64;
        (*ctx).trap_from_user = 1;
    }
    allocate_user_slot(proc_idx, kernel_stack_top, switch_sp)
}

pub fn add_user_process(
    entry: usize,
    user_stack_top: usize,
    kernel_stack_top: usize,
    user_page_table: usize,
    proc_idx: usize,
) -> Option<usize> {
    spawn_user_task(
        proc_idx,
        UserTaskInit {
            entry,
            user_stack_top,
            kernel_stack_top,
            user_page_table,
        },
    )
    .ok()
}

#[cfg(target_arch = "x86_64")]
pub fn spawn_clone_task(proc_idx: usize, init: CloneTaskInit<'_>) -> Result<usize, SchedError> {
    let initial_sp = build_clone_stack(init);
    let slot = allocate_user_slot(proc_idx, init.kernel_stack_top, initial_sp)?;
    set_task_fs_base(slot, init.tls as u64);
    PENDING_RSP0.store(init.kernel_stack_top as u64, Ordering::Relaxed);
    Ok(slot)
}

#[cfg(target_arch = "x86_64")]
pub fn add_clone_process(
    parent_ctx: &crate::arch::trap::TrapContext,
    new_user_sp: usize,
    kernel_stack_top: usize,
    user_cr3: usize,
    proc_idx: usize,
    tls: usize,
) -> Option<usize> {
    spawn_clone_task(
        proc_idx,
        CloneTaskInit {
            parent_ctx,
            new_user_sp,
            kernel_stack_top,
            user_cr3,
            tls,
        },
    )
    .ok()
}

pub fn start_first_task() -> ! {
    crate::console_println!("[sched] Starting first task...");
    let next = {
        let mut sched = SCHEDULER.lock();
        let next = find_next_ready_user(&mut sched, IDLE_SLOT).expect("no initial user task");
        if let Some(n) = node_mut(&mut sched, IDLE_SLOT) {
            n.state = TaskState::Running;
        }
        if let Some(n) = node_mut(&mut sched, next) {
            n.state = TaskState::Running;
        }
        sched.current = next;
        LAST_SCHEDULED.store(next, Ordering::Relaxed);
        next
    };
    crate::console_println!("[sched] Switching to next task: {}", next);
    switch_to(IDLE_SLOT, next);
    idle_loop()
}

fn idle_loop() -> ! {
    loop {
        // Timer/IRQ handlers may wake blocked tasks while the BSP is parked in
        // idle. Re-run the scheduler before halting again so Ready tasks resume.
        schedule();

        #[cfg(target_arch = "x86_64")]
        x86_64::instructions::interrupts::enable_and_hlt();

        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

pub fn remove_task(proc_idx: usize) {
    mark_task_exited_by_proc(proc_idx);
}

/// Change a task's scheduling class (P1.1 sys_setpriority backend).
/// Re-queues at the new priority if the task is currently ready.
pub fn set_task_class(proc_idx: usize, class: SchedClass) -> bool {
    let slot = proc_slot_get(proc_idx);
    if slot == NO_SLOT {
        return false;
    }
    let mut sched = SCHEDULER.lock();
    let was_ready;
    if let Some(n) = node_mut(&mut sched, slot) {
        if matches!(n.kind, TaskKind::Empty) {
            return false;
        }
        n.class = class;
        n.quantum_left = class.quantum();
        was_ready = n.state == TaskState::Ready;
        if was_ready {
            sched.ready.remove(slot);
        }
        if was_ready {
            let prio = class.priority();
            sched.ready.push(slot, prio);
        }
    } else {
        return false;
    }
    true
}

/// Read a task's scheduling class (P1.1 sys_getscheduler backend).
pub fn get_task_class(proc_idx: usize) -> Option<SchedClass> {
    let slot = proc_slot_get(proc_idx);
    if slot == NO_SLOT {
        return None;
    }
    let sched = SCHEDULER.lock();
    node_ref(&sched, slot).map(|n| n.class)
}

pub fn get_task_slot(proc_idx: usize) -> usize {
    proc_slot_get(proc_idx)
}

#[cfg(target_arch = "x86_64")]
pub fn task_kernel_stack(slot: usize) -> u64 {
    let sched = SCHEDULER.lock();
    node_ref(&sched, slot)
        .map(|n| n.kstack.load(Ordering::Relaxed))
        .unwrap_or(0)
}

#[cfg(target_arch = "x86_64")]
pub fn set_task_fs_base(slot: usize, val: u64) {
    let mut sched = SCHEDULER.lock();
    if let Some(n) = node_mut(&mut sched, slot) {
        n.fs_base.store(val, Ordering::Relaxed);
    }
}

/// Typed version: set FS_BASE using the FsBase newtype.
#[cfg(target_arch = "x86_64")]
pub fn set_task_fs_base_typed(slot: usize, val: crate::arch::user_return::FsBase) {
    set_task_fs_base(slot, val.raw());
}

/// Typed version: get FS_BASE as the FsBase newtype.
#[cfg(target_arch = "x86_64")]
pub fn get_task_fs_base_typed(slot: usize) -> crate::arch::user_return::FsBase {
    crate::arch::user_return::FsBase::new(get_task_fs_base(slot))
}

#[cfg(target_arch = "x86_64")]
pub fn get_task_fs_base(slot: usize) -> u64 {
    let sched = SCHEDULER.lock();
    node_ref(&sched, slot)
        .map(|n| n.fs_base.load(Ordering::Relaxed))
        .unwrap_or(0)
}

#[cfg(not(target_arch = "x86_64"))]
pub fn get_task_fs_base(_slot: usize) -> u64 {
    0
}

#[cfg(target_arch = "x86_64")]
pub fn set_pending_rsp0(val: u64) {
    PENDING_RSP0.store(val, Ordering::Relaxed);
}

#[cfg(target_arch = "x86_64")]
pub fn pending_rsp0() -> u64 {
    PENDING_RSP0.load(Ordering::Relaxed)
}

pub fn set_task_sp(slot: usize, sp: usize) {
    let mut sched = SCHEDULER.lock();
    if let Some(n) = node_mut(&mut sched, slot) {
        n.sp.store(sp, Ordering::Relaxed);
    }
}

pub fn task_sp(slot: usize) -> usize {
    let sched = SCHEDULER.lock();
    node_ref(&sched, slot)
        .map(|n| n.sp.load(Ordering::Relaxed))
        .unwrap_or(0)
}

pub fn child_count() -> usize {
    let sched = SCHEDULER.lock();
    sched
        .nodes
        .iter()
        .filter(|n| {
            n.as_deref()
                .map(|node| matches!(node.kind, TaskKind::User { .. }))
                .unwrap_or(false)
        })
        .count()
}

pub fn is_slot_active(slot: usize) -> bool {
    let sched = SCHEDULER.lock();
    node_ref(&sched, slot)
        .map(|n| !matches!(n.kind, TaskKind::Empty))
        .unwrap_or(false)
}

pub fn slot_to_process(slot: usize) -> usize {
    let sched = SCHEDULER.lock();
    match node_ref(&sched, slot).map(|n| n.kind) {
        Some(TaskKind::User { proc_idx }) => proc_idx,
        _ => usize::MAX,
    }
}

pub fn set_slot_process(slot: usize, proc_idx: usize) {
    {
        let mut sched = SCHEDULER.lock();
        if let Some(n) = node_mut(&mut sched, slot) {
            n.kind = TaskKind::User { proc_idx };
        }
    }
    proc_slot_set(proc_idx, slot);
}

pub fn current_slot() -> usize {
    CURRENT_RUNNING.load(Ordering::Relaxed)
}

pub fn current_task_id() -> usize {
    current_slot()
}

pub fn set_current_brk(addr: usize) {
    crate::process::set_current_brk(addr);
}

// ─── Scheduler tests (test_mode) ──────────────────────────────────

#[cfg(target_arch = "riscv64")]
unsafe fn test_mask_interrupts() {
    core::arch::asm!("csrci sstatus, 0x2");
}
#[cfg(target_arch = "riscv64")]
unsafe fn test_restore_interrupts() {
    core::arch::asm!("csrsi sstatus, 0x2");
}
#[cfg(target_arch = "x86_64")]
unsafe fn test_mask_interrupts() {
    x86_64::instructions::interrupts::disable();
}
#[cfg(target_arch = "x86_64")]
unsafe fn test_restore_interrupts() {
    x86_64::instructions::interrupts::enable();
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    // Stress the dynamic task table (P1.1 acceptance: 200 tasks, all
    // schedulable, slots recycled). Interrupts are masked so the timer-driven
    // schedule() cannot consume the ready queue while address-less fake tasks
    // are queued (allocate only writes node fields, never task stacks).
    crate::test::run_test("sched_dynamic_200_task_stress", || {
        let mut slots: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
        unsafe { test_mask_interrupts() };
        let mut ok = true;
        for i in 0..200usize {
            match allocate_user_slot(i, 0, 0x4000_0000 + i * 0x1000) {
                Ok(s) => slots.push(s),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        // All slots unique.
        let mut sorted = slots.clone();
        sorted.sort_unstable();
        ok = ok && sorted.len() == 200 && sorted.windows(2).all(|w| w[0] != w[1]);
        // All 200 are queued ready and counted as live children.
        let ready_n = {
            let sched = SCHEDULER.lock();
            sched.ready.len()
        };
        let children = child_count();
        ok = ok && ready_n == 200 && children == 200;
        // Free them all; queue must drain.
        for i in 0..200usize {
            mark_task_exited_by_proc(i);
        }
        let ready_after = {
            let sched = SCHEDULER.lock();
            sched.ready.len()
        };
        ok = ok && ready_after == 0;
        // Slot reuse: the next allocation must come from the free list.
        let reused = allocate_user_slot(500, 0, 0x5000_0000).ok();
        ok = ok && reused.map(|s| slots.contains(&s)).unwrap_or(false);
        if reused.is_some() {
            mark_task_exited_by_proc(500);
        }
        unsafe { test_restore_interrupts() };
        ok
    });

    // RT pick-decision latency (P1.1 acceptance: measured number lands in
    // docs/benchmarks.md). Timer-tick granularity note: full RT preemption
    // latency is bounded by the tick interval; this measures the in-kernel
    // pick decision (lock + bitmap scan) that adds on top of it.
    let t0 = test_cycles();
    for _ in 0..1000 {
        let mut sched = SCHEDULER.lock();
        let _ = sched.ready.pop_next();
    }
    let t1 = test_cycles();
    let per_pick = (t1 - t0) / 1000;
    crate::console_println!("[bench] sched_pick_empty_avg_cycles={}", per_pick);
}

#[cfg(all(target_arch = "riscv64", feature = "test_mode"))]
fn test_cycles() -> u64 {
    let t: u64;
    unsafe { core::arch::asm!("rdcycle {}", out(reg) t) };
    t
}

#[cfg(all(target_arch = "x86_64", feature = "test_mode"))]
fn test_cycles() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

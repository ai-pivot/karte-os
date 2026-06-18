// kernel/src/sched/mod.rs — unified task scheduler.
//
// Design:
//   - The scheduler knows tasks, not "init". Shell is just the first User task.
//   - A kernel Idle task is the only fallback when no user task is runnable.
//   - Every User task has a valid saved_sp before it can be scheduled.

pub mod task;

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::sync::spinlock::SpinLock;
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

pub const MAX_TASKS: usize = 64;

const IDLE_SLOT: usize = 0;
const NO_SLOT: usize = MAX_TASKS;

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

#[derive(Clone, Copy)]
pub struct CloneTaskInit<'a> {
    pub parent_ctx: &'a crate::arch::trap::TrapContext,
    pub new_user_sp: usize,
    pub kernel_stack_top: usize,
    pub user_page_table: usize,
    pub tls: usize,
}

static TASK_SPS: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(0) }; MAX_TASKS];
static INITIAL_TASK_SP: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(0) }; MAX_TASKS];
static PROC_TO_SLOT: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(NO_SLOT) }; MAX_TASKS];

/// Currently running scheduler slot. Slot 0 is the typed Idle task, not init.
pub static CURRENT_RUNNING: AtomicUsize = AtomicUsize::new(IDLE_SLOT);

/// Flag set by timer ISR when a schedule is needed.
/// Checked and acted upon in the ecall return path.
/// This prevents reentrant schedule() from timer ISR, which causes
/// lock-holder preemption deadlocks (task A holds spinlock,
/// timer switches to task B, task B spins on same lock forever).
pub static NEED_RESCHED: AtomicBool = AtomicBool::new(false);

static LAST_SCHEDULED: AtomicUsize = AtomicUsize::new(IDLE_SLOT);

#[cfg(target_arch = "x86_64")]
pub(crate) static TASK_KSTACK: [core::sync::atomic::AtomicU64; MAX_TASKS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_TASKS];

#[cfg(target_arch = "x86_64")]
static TASK_FS_BASE: [core::sync::atomic::AtomicU64; MAX_TASKS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_TASKS];

#[cfg(target_arch = "x86_64")]
static PENDING_RSP0: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

struct Scheduler {
    tasks: [Option<TaskControlBlock>; MAX_TASKS],
    kinds: [TaskKind; MAX_TASKS],
    current: usize,
    high_water: usize,
}

static SCHEDULER: SpinLock<Scheduler> = SpinLock::new(Scheduler {
    tasks: [const { None }; MAX_TASKS],
    kinds: [TaskKind::Empty; MAX_TASKS],
    current: IDLE_SLOT,
    high_water: 1,
});

pub fn init() {
    let mut sched = SCHEDULER.lock();
    sched.tasks[IDLE_SLOT] = Some(TaskControlBlock::new_idle(IDLE_SLOT));
    sched.kinds[IDLE_SLOT] = TaskKind::Idle;
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
    match sched.kinds[sched.current] {
        TaskKind::User { proc_idx } => Some(proc_idx),
        _ => None,
    }
}

#[cfg(target_arch = "x86_64")]
pub fn current_kernel_stack() -> Option<u64> {
    let current = CURRENT_RUNNING.load(Ordering::Relaxed);
    if current < MAX_TASKS {
        let ksp = TASK_KSTACK[current].load(Ordering::Relaxed);
        if ksp != 0 {
            return Some(ksp);
        }
    }
    None
}

fn find_next_ready_user(sched: &Scheduler, current: usize) -> Option<usize> {
    let start = LAST_SCHEDULED.load(Ordering::Relaxed).wrapping_add(1);
    for i in 0..MAX_TASKS {
        let candidate = (start + i) % MAX_TASKS;
        if candidate == current {
            continue;
        }
        if matches!(sched.kinds[candidate], TaskKind::User { .. }) {
            if let Some(ref task) = sched.tasks[candidate] {
                if task.state == TaskState::Ready {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn set_current_process_for_slot(slot: usize) {
    let kind = {
        let sched = SCHEDULER.lock();
        sched.kinds[slot]
    };
    match kind {
        TaskKind::User { proc_idx } => {
            crate::process::set_current_index(proc_idx);
            crate::process::set_current_page_table_root(crate::process::get_page_table_root(
                proc_idx,
            ));
        }
        TaskKind::Idle | TaskKind::Empty => {
            crate::process::set_current_page_table_root(0);
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn save_fs_base(slot: usize) {
    // Read the CURRENT hardware FS_BASE from MSR and save it.
    if slot >= MAX_TASKS {
        return;
    }
    let fs_base = unsafe { crate::arch::idt::rdmsr(0xC0000100) };
    TASK_FS_BASE[slot].store(fs_base, Ordering::Relaxed);
}

#[cfg(not(target_arch = "x86_64"))]
fn save_fs_base(_slot: usize) {}

#[cfg(target_arch = "x86_64")]
fn restore_task_arch_state(slot: usize) {
    if slot >= MAX_TASKS {
        return;
    }
    let kernel_sp = TASK_KSTACK[slot].load(Ordering::Relaxed);
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
    let fs_base = TASK_FS_BASE[slot].load(Ordering::Relaxed);
    unsafe { crate::arch::idt::wrmsr(0xC0000100, fs_base) };
}

#[cfg(not(target_arch = "x86_64"))]
fn restore_task_arch_state(slot: usize) {
    // RISC-V: Update CURRENT_PAGE_TABLE_ROOT so that trap_handler's tail
    // can restore the correct satp when returning to U-mode.
    // Without this, the page table root stays stale from the previous task,
    // causing the resumed task to run with the wrong address space.
    let proc_idx = match PROC_TO_SLOT[slot].load(Ordering::Relaxed) {
        NO_SLOT => return,
        idx => idx,
    };
    let root = crate::process::get_page_table_root(proc_idx);
    if root != 0 {
        crate::process::set_current_page_table_root(root);
    }
}

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

    let fs_base = FsBase::new(TASK_FS_BASE[slot].load(Ordering::Relaxed));
    let kernel_sp = TASK_KSTACK[slot].load(Ordering::Relaxed);

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
        fs_base,
    }
}

fn switch_to(current: usize, next: usize) {
    save_fs_base(current);
    CURRENT_RUNNING.store(next, Ordering::Relaxed);
    set_current_process_for_slot(next);

    #[cfg(target_arch = "x86_64")]
    {
        let next_fs_base = TASK_FS_BASE[next].load(Ordering::Relaxed);
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

    let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
    let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
    unsafe {
        __switch(cur_ptr, nxt_ptr);
    }

    let resumed = CURRENT_RUNNING.load(Ordering::Relaxed);
    restore_task_arch_state(resumed);
}

pub fn schedule() {
    let (current, next) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        let next = match find_next_ready_user(&sched, current) {
            Some(slot) => slot,
            None => return, // No Ready task — caller continues
        };

        if matches!(sched.kinds[current], TaskKind::User { .. }) {
            if let Some(ref mut task) = sched.tasks[current] {
                if task.state == TaskState::Running {
                    task.state = TaskState::Ready;
                }
            }
        }
        if let Some(ref mut task) = sched.tasks[next] {
            task.state = TaskState::Running;
        }
        sched.current = next;
        LAST_SCHEDULED.store(next, Ordering::Relaxed);
        (current, next)
    };

    switch_to(current, next);
}

pub fn schedule_block() {
    let (current, next) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        if !matches!(sched.kinds[current], TaskKind::User { .. }) {
            return;
        }
        if let Some(ref mut task) = sched.tasks[current] {
            task.state = TaskState::Blocked;
        }

        let next = find_next_ready_user(&sched, current).unwrap_or(IDLE_SLOT);
        if let Some(ref mut task) = sched.tasks[next] {
            task.state = TaskState::Running;
        }
        sched.current = next;
        LAST_SCHEDULED.store(next, Ordering::Relaxed);
        (current, next)
    };

    switch_to(current, next);
}

pub fn schedule_exit() {
    remove_sleep(CURRENT_RUNNING.load(Ordering::Relaxed));
    let (current, next) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        if let TaskKind::User { proc_idx } = sched.kinds[current] {
            PROC_TO_SLOT[proc_idx].store(NO_SLOT, Ordering::Relaxed);
        }
        sched.tasks[current] = None;
        sched.kinds[current] = TaskKind::Empty;

        let next = find_next_ready_user(&sched, current).unwrap_or(IDLE_SLOT);
        if let Some(ref mut task) = sched.tasks[next] {
            task.state = TaskState::Running;
        }
        sched.current = next;
        LAST_SCHEDULED.store(next, Ordering::Relaxed);
        (current, next)
    };

    switch_to(current, next);
}

/// Kept for syscall shutdown policy. PID 1 is the init process; it is not
/// identified by a scheduler slot.
pub fn is_init_running() -> bool {
    crate::process::current_pid() == 1
}

pub fn mark_current_exited() {
    remove_sleep(CURRENT_RUNNING.load(Ordering::Relaxed));

    // Also kill ALL scheduler slots that share the same proc_idx.
    // clone() children share the parent's proc_idx but have separate
    // scheduler slots. Without this, orphaned clone threads keep running
    // after the thread group leader exits, causing page faults on freed memory.
    let proc_idx = {
        let sched = SCHEDULER.lock();
        match sched.kinds[sched.current] {
            TaskKind::User { proc_idx } => Some(proc_idx),
            _ => None,
        }
    };
    if let Some(pi) = proc_idx {
        let mut sched = SCHEDULER.lock();
        for slot in 0..sched.high_water {
            if matches!(sched.kinds[slot], TaskKind::User { proc_idx: p } if p == pi) {
                remove_sleep(slot);
                sched.tasks[slot] = None;
                sched.kinds[slot] = TaskKind::Empty;
            }
        }
    } else {
        let mut sched = SCHEDULER.lock();
        let cur = sched.current;
        if let Some(ref mut task) = sched.tasks[cur] {
            task.state = TaskState::Exited;
        }
    }
}

pub fn mark_task_exited_by_proc(proc_idx: usize) {
    let slot = PROC_TO_SLOT[proc_idx].load(Ordering::Relaxed);
    if slot >= MAX_TASKS {
        return;
    }
    remove_sleep(slot);
    let mut sched = SCHEDULER.lock();
    sched.tasks[slot] = None;
    sched.kinds[slot] = TaskKind::Empty;
    PROC_TO_SLOT[proc_idx].store(NO_SLOT, Ordering::Relaxed);
}

/// Diagnostic: dump all task states (called periodically from timer ISR).
pub fn dump_task_states() {
    let sched = SCHEDULER.lock();
    let current = CURRENT_RUNNING.load(Ordering::Relaxed);
    let mut states = alloc::string::String::new();
    for i in 0..sched.high_water {
        if let Some(ref task) = sched.tasks[i] {
            let state_str = match task.state {
                TaskState::Ready => "R",
                TaskState::Running => "*",
                TaskState::Blocked => "B",
                TaskState::Exited => "X",
            };
            let _ = core::fmt::write(&mut states, format_args!(" s{}={} ", i, state_str));
        }
    }
    drop(sched);
    crate::klog!(INFO, "[diag] cur={}{}", current, states);
}

/// Wake a task directly by scheduler slot (bypasses PROC_TO_SLOT).
/// Needed for clone threads that share proc_idx but have unique slots.
pub fn wake_task_by_slot(slot: usize) -> bool {
    if slot >= MAX_TASKS {
        return false;
    }
    remove_sleep(slot);
    let mut sched = SCHEDULER.lock();
    if let Some(ref mut task) = sched.tasks[slot] {
        if task.state == TaskState::Blocked {
            task.state = TaskState::Ready;
            return true;
        }
    }
    false
}

pub fn wake_task(proc_idx: usize) -> bool {
    let slot = PROC_TO_SLOT[proc_idx].load(Ordering::Relaxed);
    if slot >= MAX_TASKS {
        return false;
    }
    remove_sleep(slot);
    let mut sched = SCHEDULER.lock();
    if let Some(ref mut task) = sched.tasks[slot] {
        if task.state == TaskState::Blocked {
            task.state = TaskState::Ready;
            return true;
        }
    }
    false
}

const MAX_SLEEPQ: usize = 32;
static SLEEPQ: SpinLock<[(usize, u64); MAX_SLEEPQ]> = SpinLock::new([(NO_SLOT, 0u64); MAX_SLEEPQ]);
static SLEEPQ_LEN: AtomicUsize = AtomicUsize::new(0);

fn remove_sleep(slot: usize) {
    let mut q = SLEEPQ.lock();
    let len = SLEEPQ_LEN.load(Ordering::Relaxed);
    let mut new_len = 0;
    for i in 0..len {
        if q[i].0 == slot {
            continue;
        }
        if new_len != i {
            q[new_len] = q[i];
        }
        new_len += 1;
    }
    for i in new_len..len {
        q[i] = (NO_SLOT, 0);
    }
    SLEEPQ_LEN.store(new_len, Ordering::Relaxed);
}

fn queue_sleep(slot: usize, wake_tick: u64) -> bool {
    let mut q = SLEEPQ.lock();
    let len = SLEEPQ_LEN.load(Ordering::Relaxed);
    for i in 0..len {
        if q[i].0 == slot {
            q[i].1 = wake_tick;
            return true;
        }
    }
    if len < MAX_SLEEPQ {
        q[len] = (slot, wake_tick);
        SLEEPQ_LEN.store(len + 1, Ordering::Relaxed);
        true
    } else {
        false
    }
}

pub fn sleep_until(wake_tick: u64) {
    let now = crate::arch::platform::uptime_ms();
    if wake_tick <= now {
        return;
    }
    let slot = CURRENT_RUNNING.load(Ordering::Relaxed);
    if slot >= MAX_TASKS || !is_slot_active(slot) {
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

pub fn tick_sleep_queue() {
    // Fast path: skip if no sleeping tasks
    if SLEEPQ_LEN.load(Ordering::Relaxed) == 0 {
        return;
    }
    let now = crate::arch::platform::uptime_ms();
    let mut to_wake = [NO_SLOT; MAX_SLEEPQ];
    let mut wake_len = 0usize;
    {
        let mut q = match SLEEPQ.try_lock() {
            Some(guard) => guard,
            None => return,
        };
        let len = SLEEPQ_LEN.load(Ordering::Relaxed);
        let mut new_len = 0;
        for i in 0..len {
            let (slot, wake_tick) = q[i];
            if slot != NO_SLOT && now >= wake_tick {
                if wake_len < MAX_SLEEPQ {
                    to_wake[wake_len] = slot;
                    wake_len += 1;
                }
            } else {
                if new_len != i {
                    q[new_len] = q[i];
                }
                new_len += 1;
            }
        }
        SLEEPQ_LEN.store(new_len, Ordering::Relaxed);
    }

    if wake_len == 0 {
        return;
    }
    let mut sched = match SCHEDULER.try_lock() {
        Some(guard) => guard,
        None => return,
    };
    for &slot in &to_wake[..wake_len] {
        if let Some(ref mut task) = sched.tasks[slot] {
            if task.state == TaskState::Blocked {
                task.state = TaskState::Ready;
            }
        }
    }
}

#[cfg(target_arch = "riscv64")]
fn build_initial_stack(init: UserTaskInit) -> usize {
    let ctx_size = core::mem::size_of::<crate::arch::trap::TrapContext>();
    let trap_ctx_base = init.kernel_stack_top - ctx_size;
    let switch_sp = trap_ctx_base - 112;
    unsafe {
        core::ptr::write_bytes(switch_sp as *mut u8, 0, ctx_size + 112);
        let sw = switch_sp as *mut usize;
        *sw.add(0) = first_task_shim as *const () as usize;
        let ctx = trap_ctx_base as *mut usize;
        *ctx.add(2) = init.user_stack_top;
        *ctx.add(32) = 0x20 | (1 << 13);
        *ctx.add(33) = init.entry;
        *ctx.add(34) = init.user_stack_top;
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
        ctx.user_cr3 = init.user_page_table as u64;
        ctx.trap_from_user = 1;
        core::ptr::write(trap_ctx_base as *mut crate::arch::trap::TrapContext, ctx);
    }
    switch_sp
}

/// Build initial kernel stack for a clone()'d thread (RISC-V).
/// Copies the parent's TrapContext, then modifies:
///   a0=0 (child return), sp=child_stack, tp=TLS, sepc+=4 (skip ecall)
#[cfg(target_arch = "riscv64")]
fn build_clone_stack(init: CloneTaskInit<'_>) -> usize {
    let ctx_size = core::mem::size_of::<crate::arch::trap::TrapContext>();
    let trap_ctx_base = init.kernel_stack_top - ctx_size;
    let switch_sp = trap_ctx_base - 112; // 14 callee-saved regs = 112 bytes

    unsafe {
        core::ptr::write_bytes(switch_sp as *mut u8, 0, ctx_size + 112);

        // Set up __switch return frame: jump to first_task_shim
        let sw = switch_sp as *mut usize;
        *sw.add(0) = first_task_shim as *const () as usize;

        // Copy parent's TrapContext to child's stack
        let ctx = trap_ctx_base as *mut usize;
        let parent = init.parent_ctx as *const _ as *const usize;
        for i in 0..36 {
            // 36 usize fields: x[0..32], sstatus, sepc, sscratch, user_satp
            *ctx.add(i) = *parent.add(i);
        }

        // Modify child's TrapContext for thread entry:
        *ctx.add(10) = 0; // x[10] (a0) = 0: child clone() returns 0
        *ctx.add(2) = init.new_user_sp; // x[2] (sp) = child user stack
        // Set tp (x[4]) from TLS when CLONE_SETTLS was specified.
        // On Linux RISC-V, clone passes tls in a4 and the kernel must set tp.
        // Go runtime relies on tp pointing to its TLS block (which contains g).
        if init.tls != 0 {
            *ctx.add(4) = init.tls; // x[4] (tp) = TLS pointer
        }
        *ctx.add(34) = init.new_user_sp; // sscratch = child user sp
        let parent_sepc = *ctx.add(33);
        *ctx.add(33) = parent_sepc + 4; // sepc += 4: skip ecall instruction
        *ctx.add(35) = init.user_page_table; // user_satp = shared page table
    }

    switch_sp
}

fn allocate_user_slot(
    proc_idx: usize,
    kernel_stack_top: usize,
    initial_sp: usize,
) -> Result<usize, SchedError> {
    let mut sched = SCHEDULER.lock();
    let slot = (0..MAX_TASKS)
        .find(|&i| sched.kinds[i] == TaskKind::Empty)
        .ok_or(SchedError::NoFreeSlot)?;

    sched.tasks[slot] = Some(TaskControlBlock::new(slot));
    sched.kinds[slot] = TaskKind::User { proc_idx };
    if let Some(ref mut task) = sched.tasks[slot] {
        task.state = TaskState::Ready;
    }
    if slot >= sched.high_water {
        sched.high_water = slot + 1;
    }
    PROC_TO_SLOT[proc_idx].store(slot, Ordering::Relaxed);
    TASK_SPS[slot].store(initial_sp, Ordering::Relaxed);
    INITIAL_TASK_SP[slot].store(initial_sp, Ordering::Relaxed);

    #[cfg(target_arch = "x86_64")]
    {
        TASK_KSTACK[slot].store(kernel_stack_top as u64, Ordering::Relaxed);
        TASK_FS_BASE[slot].store(0, Ordering::Relaxed);
    }

    Ok(slot)
}

pub fn spawn_user_task(proc_idx: usize, init: UserTaskInit) -> Result<usize, SchedError> {
    let initial_sp = build_initial_stack(init);
    allocate_user_slot(proc_idx, init.kernel_stack_top, initial_sp)
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

pub fn spawn_clone_task(proc_idx: usize, init: CloneTaskInit<'_>) -> Result<usize, SchedError> {
    let initial_sp = build_clone_stack(init);
    let slot = allocate_user_slot(proc_idx, init.kernel_stack_top, initial_sp)?;
    #[cfg(target_arch = "x86_64")]
    {
        TASK_FS_BASE[slot].store(init.tls as u64, Ordering::Relaxed);
        PENDING_RSP0.store(init.kernel_stack_top as u64, Ordering::Relaxed);
    }
    let _ = init.tls;
    Ok(slot)
}

pub fn add_clone_process(
    parent_ctx: &crate::arch::trap::TrapContext,
    new_user_sp: usize,
    kernel_stack_top: usize,
    user_page_table: usize,
    proc_idx: usize,
    tls: usize,
) -> Option<usize> {
    spawn_clone_task(
        proc_idx,
        CloneTaskInit {
            parent_ctx,
            new_user_sp,
            kernel_stack_top,
            user_page_table,
            tls,
        },
    )
    .ok()
}

pub fn start_first_task() -> ! {
    crate::console_println!("[sched] Starting first task...");
    let next = {
        let mut sched = SCHEDULER.lock();
        let next = find_next_ready_user(&sched, IDLE_SLOT).expect("no initial user task");
        if let Some(ref mut task) = sched.tasks[IDLE_SLOT] {
            task.state = TaskState::Running;
        }
        if let Some(ref mut task) = sched.tasks[next] {
            task.state = TaskState::Running;
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
        schedule();

        // Use idle time to pre-zero pages for the PF handler
        crate::mm::pmm::refill_zeroed_pool();

        #[cfg(target_arch = "x86_64")]
        x86_64::instructions::interrupts::enable_and_hlt();

        #[cfg(target_arch = "riscv64")]
        unsafe {
            // Enable S-mode interrupts so the timer can wake us from wfi.
            //
            // When schedule_block() switches to IDLE, we arrive here with
            // SIE=0 (trap entry cleared it). Without setting SIE, wfi never
            // wakes because there is no pending *enabled* interrupt, causing
            // a system-wide deadlock when all user tasks are blocked.
            //
            // This is safe: schedule() has returned (no SCHEDULER lock held),
            // and the timer ISR uses try_lock for all kernel locks, so even
            // if it fires between set_sie and wfi there is no deadlock.
            riscv::register::sstatus::set_sie();
            core::arch::asm!("wfi");
            riscv::register::sstatus::clear_sie();
        }
    }
}

pub fn remove_task(proc_idx: usize) {
    mark_task_exited_by_proc(proc_idx);
}

pub fn get_task_slot(proc_idx: usize) -> usize {
    PROC_TO_SLOT[proc_idx].load(Ordering::Relaxed)
}

#[cfg(target_arch = "x86_64")]
pub fn task_kernel_stack(slot: usize) -> u64 {
    TASK_KSTACK[slot].load(Ordering::Relaxed)
}

#[cfg(target_arch = "x86_64")]
pub fn set_task_fs_base(slot: usize, val: u64) {
    if slot < MAX_TASKS {
        TASK_FS_BASE[slot].store(val, Ordering::Relaxed);
    }
}

/// Typed version: set FS_BASE using the FsBase newtype.
#[cfg(target_arch = "x86_64")]
pub fn set_task_fs_base_typed(slot: usize, val: crate::arch::user_return::FsBase) {
    if slot < MAX_TASKS {
        TASK_FS_BASE[slot].store(val.raw(), Ordering::Relaxed);
    }
}

/// Typed version: get FS_BASE as the FsBase newtype.
#[cfg(target_arch = "x86_64")]
pub fn get_task_fs_base_typed(slot: usize) -> crate::arch::user_return::FsBase {
    crate::arch::user_return::FsBase::new(get_task_fs_base(slot))
}

#[cfg(target_arch = "x86_64")]
pub fn get_task_fs_base(slot: usize) -> u64 {
    if slot < MAX_TASKS {
        TASK_FS_BASE[slot].load(Ordering::Relaxed)
    } else {
        0
    }
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
    if slot < MAX_TASKS {
        TASK_SPS[slot].store(sp, Ordering::Relaxed);
    }
}

pub fn task_sp(slot: usize) -> usize {
    if slot < MAX_TASKS {
        TASK_SPS[slot].load(Ordering::Relaxed)
    } else {
        0
    }
}

pub fn child_count() -> usize {
    let sched = SCHEDULER.lock();
    sched
        .kinds
        .iter()
        .filter(|kind| matches!(kind, TaskKind::User { .. }))
        .count()
}

pub fn is_slot_active(slot: usize) -> bool {
    let sched = SCHEDULER.lock();
    slot < MAX_TASKS && sched.tasks[slot].is_some()
}

pub fn slot_to_process(slot: usize) -> usize {
    let sched = SCHEDULER.lock();
    match sched.kinds[slot] {
        TaskKind::User { proc_idx } => proc_idx,
        _ => usize::MAX,
    }
}

pub fn set_slot_process(slot: usize, proc_idx: usize) {
    let mut sched = SCHEDULER.lock();
    if slot < MAX_TASKS {
        sched.kinds[slot] = TaskKind::User { proc_idx };
        PROC_TO_SLOT[proc_idx].store(slot, Ordering::Relaxed);
    }
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

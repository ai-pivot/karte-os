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

#[cfg(target_arch = "riscv64")]
global_asm!(include_str!("../arch/riscv64/switch.S"));

// Shim for first task entry via __switch.
// __switch already pops its own 104-byte frame before `ret`ing here, so sp
// already points at the TrapContext. Jump straight into the U-mode return path,
// which will switch satp (via TrapContext.user_satp) and sret into the task.
#[cfg(target_arch = "riscv64")]
global_asm!(
    ".globl first_task_shim",
    "first_task_shim:",
    "j trap_return_user",
);

// On x86_64, switch.S is included from arch/x86_64/switch.rs via global_asm!
// and __switch is declared there. first_task_shim is defined below as a
// Rust naked function that jumps to trap_return_user.

#[cfg(target_arch = "x86_64")]
unsafe extern "C" {
    fn __switch(current_sp: *mut usize, next_sp: *const usize);
}

#[cfg(target_arch = "riscv64")]
unsafe extern "C" {
    fn __switch(current_sp: *mut usize, next_sp: *const usize);
    fn first_task_shim();
}

/// First task shim for x86_64: __switch `ret`s here.
/// After __switch, rsp points at the TrapContext.
/// We pass rsp as argument to trap_return_user, which pops GP regs and iretqs.
/// MUST be naked — any compiler prologue would corrupt rsp.
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

#[cfg(target_arch = "x86_64")]
unsafe extern "C" {
    fn trap_return_user(ctx: *mut crate::arch::trap::TrapContext) -> !;
}

pub const MAX_TASKS: usize = 64;

/// Sentinel value for `Scheduler::current` meaning "init (the shell) is running".
/// Init is NOT a TaskControlBlock; its saved kernel sp lives in INIT_TASK_SP.
const INIT_SENTINEL: usize = MAX_TASKS;

/// PROCESS_TABLE index of the init process (the shell).
const INIT_PROC_IDX: usize = 0;

static TASK_SPS: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(0) }; MAX_TASKS];
/// Which task slot is currently running (updated by schedule() before __switch).
/// After __switch returns, this reflects the task we just switched TO
/// (may have been updated by another schedule() invocation in between).
pub static CURRENT_RUNNING: AtomicUsize = AtomicUsize::new(MAX_TASKS);

/// Get the currently running scheduler slot (for diagnostics).
pub fn current_running_slot() -> usize {
    CURRENT_RUNNING.load(Ordering::Relaxed)
}
/// Last scheduled task slot — used for fair round-robin when switching from init.
/// Without this, schedule() always picks slot 0 when init is running,
/// starving higher-numbered slots.
static LAST_SCHEDULED: AtomicUsize = AtomicUsize::new(0);
/// Mapping from process table index to scheduler slot.
/// Needed because process_idx != scheduler_slot in general.
static PROC_TO_SLOT: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(MAX_TASKS) }; MAX_TASKS];
/// Initial TASK_SPS values saved by add_user_process / add_clone_process.
/// Used by schedule_exit() to detect tasks that were never context-switched out.
static INITIAL_TASK_SP: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(0) }; MAX_TASKS];

/// Per-task kernel stack tops for RSP0/TSS update in schedule().
/// Stored when task is created (add_user_process/add_clone_process).
#[cfg(target_arch = "x86_64")]
pub(crate) static TASK_KSTACK: [core::sync::atomic::AtomicU64; MAX_TASKS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_TASKS];

/// Get the kernel stack top of the currently scheduled task.
/// Returns None if init (shell) is running (init has no TCB).
#[cfg(target_arch = "x86_64")]
pub fn current_kernel_stack() -> Option<u64> {
    let sched = SCHEDULER.lock();
    if sched.current < MAX_TASKS {
        Some(TASK_KSTACK[sched.current].load(Ordering::Relaxed))
    } else {
        None
    }
}

/// Pending RSP0 value for clone_first_shim (set by add_clone_process).
/// This avoids the need to read TrapContext.kernel_sp which may have
/// incorrect offset due to __switch RSP alignment.
#[cfg(target_arch = "x86_64")]
static PENDING_RSP0: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Per-task FS_BASE values for TLS. Lock-free, used in Timer ISR for wrmsr restore.
#[cfg(target_arch = "x86_64")]
static TASK_FS_BASE: [core::sync::atomic::AtomicU64; MAX_TASKS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_TASKS];

/// Saved kernel sp for init. When schedule() switches from init to a child,
/// __switch saves init's sp here. schedule_exit() uses it to switch back.
static INIT_TASK_SP: AtomicUsize = AtomicUsize::new(0);

/// Log counter — only print first N scheduler events to avoid flooding.
static SCHED_LOG_COUNT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
const SCHED_LOG_LIMIT: usize = 2000;

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

pub fn init() {}

/// Round-Robin among child tasks. When init is running
/// (current == INIT_SENTINEL), __switch saves init's sp to INIT_TASK_SP then
/// switches to the next Ready child. When a child is running, rotate to the
/// next Ready child after it (if any). Returning to init is handled by
/// schedule_exit() when a child exits, not here.
pub fn schedule() {
    let (switch_from_init, current, next, next_proc_idx) = {
        let mut sched = SCHEDULER.lock();
        if sched.count == 0 {
            return;
        }
        let current = sched.current;
        let init_running = current == INIT_SENTINEL;
        let count = sched.count;

        // Find the next Ready child. When init is running, scan from slot 0;
        // when a child is running, scan starting after it (round-robin).
        // Use LAST_SCHEDULED for fairness when switching from init.
        let start = if init_running {
            LAST_SCHEDULED.load(Ordering::Relaxed) + 1
        } else {
            current + 1
        };
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

        // Scheduler log (limited)
        {
            let n = SCHED_LOG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < SCHED_LOG_LIMIT {
                if init_running {
                    crate::console_println!("[sched] #{} timer: init -> slot{}", n, next);
                } else {
                    crate::console_println!(
                        "[sched] #{} timer: slot{} -> slot{}",
                        n,
                        current,
                        next
                    );
                }
            }
        }

        // Remember which slot we picked for fair round-robin next time
        LAST_SCHEDULED.store(next, Ordering::Relaxed);

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
        (init_running, current, next, next_proc_idx)
    };

    // Save current task's FS_BASE before context switch
    // Note: init has no slot, don't save when switching from init
    #[cfg(target_arch = "x86_64")]
    if !switch_from_init {
        let save_idx = current;
        if save_idx < MAX_TASKS {
            let fs_base: u64;
            unsafe {
                core::arch::asm!(
                    "rdmsr",
                    "shl rdx, 32",
                    "or rax, rdx",
                    out("rax") fs_base,
                    out("rdx") _,
                    in("ecx") 0xC0000100u32,
                );
            }
            TASK_FS_BASE[save_idx].store(fs_base, Ordering::Relaxed);
        }
    }

    if switch_from_init {
        // Save init's sp to INIT_TASK_SP, switch to child
        let init_sp_ptr = INIT_TASK_SP.as_ptr() as *mut usize;
        let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
        let _next_sp_val = unsafe { *nxt_ptr };

        // Set PENDING_FS_BASE for trap_return_user (same as non-init path)
        #[cfg(target_arch = "x86_64")]
        {
            let next_fs_base = TASK_FS_BASE[next].load(Ordering::Relaxed);
            crate::arch::trap::PENDING_FS_BASE.store(next_fs_base, Ordering::Relaxed);
        }

        // NOTE: Do NOT change SYSCALL_KSP here! SYSCALL_KSP is used by the
        // currently executing code (init's syscall handler). Changing it would
        // cause subsequent stack pushes to land on the child's kernel stack,
        // corrupting the child's TrapContext. Instead, SYSCALL_KSP is set in
        // trap_return_user just before returning to user mode.
        unsafe {
            __switch(init_sp_ptr, nxt_ptr);
        }
        // After __switch returns (init was switched back), ensure SYSCALL_KSP
        // points to init's stack
        #[cfg(target_arch = "x86_64")]
        {
            let ksp = crate::arch::idt::INIT_SYSCALL_KSP.load(Ordering::Relaxed);
            unsafe { crate::arch::idt::SYSCALL_KSP = ksp };
            unsafe {
                crate::arch::gdt::set_kernel_rsp0_for_cpu(0, ksp);
            };
        }
    } else {
        // Mark the next task as running BEFORE __switch.
        // After __switch returns (possibly on a different invocation's stack),
        // we read CURRENT_RUNNING to know which task we actually are.
        CURRENT_RUNNING.store(next, Ordering::Relaxed);

        // Set PENDING_FS_BASE for trap_return_user.
        // When __switch resumes a task for the first time (via trap_return_user),
        // it bypasses the FS_BASE restore code below. trap_return_user reads
        // this value to set the thread's TLS correctly before returning to Ring 3.
        #[cfg(target_arch = "x86_64")]
        {
            let next_fs_base = TASK_FS_BASE[next].load(Ordering::Relaxed);
            crate::arch::trap::PENDING_FS_BASE.store(next_fs_base, Ordering::Relaxed);
        }

        let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
        let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
        unsafe {
            __switch(cur_ptr, nxt_ptr);
        }
    }

    // After __switch returns (possibly on a different task's stack),
    // use CURRENT_RUNNING to identify which task we actually are now.
    // (The local `next` variable may be stale — it was set in a different
    //  schedule() invocation before this task was switched out.)
    #[cfg(target_arch = "x86_64")]
    {
        let current = CURRENT_RUNNING.load(Ordering::Relaxed);
        let n = SCHED_LOG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < SCHED_LOG_LIMIT {
            crate::console_println!("[sched] #{} sched_resume: CURRENT_RUNNING={}", n, current);
        }
        if current < MAX_TASKS {
            let kernel_sp = TASK_KSTACK[current].load(Ordering::Relaxed);
            if kernel_sp != 0 {
                crate::arch::idt::set_syscall_ksp(kernel_sp as u64);
                // Update TSS.RSP0 — use VirtAddr::new_truncate (no panic)
                unsafe {
                    crate::arch::gdt::set_kernel_rsp0_for_cpu(0, kernel_sp as u64);
                }
            }
            // Restore FS_BASE for this task (critical for Go TLS / getg())
            let fs_base = TASK_FS_BASE[current].load(Ordering::Relaxed);
            if fs_base != 0 {
                unsafe {
                    // Write IA32_FS_BASE MSR (0xC0000100)
                    core::arch::asm!(
                        "wrmsr",
                        in("ecx") 0xC0000100u32,
                        in("edx") (fs_base >> 32) as u32,
                        in("eax") (fs_base & 0xFFFFFFFF) as u32,
                    );
                }
            }
        }
    }
}

/// Current child exits → switch to another Ready child, or back to init if
/// none remain. Called from sys_exit (current is always a child here).
pub fn schedule_exit() {
    let (switch_to_init, current, next, next_proc_idx) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        let count = sched.count;
        if current < count {
            // Free the scheduler slot — task is exiting, no need to keep it
            sched.tasks[current] = None;
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
            (true, current, next, 0)
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
            (false, current, next, next_proc_idx)
        }
    };

    // Save current task's FS_BASE before context switch
    #[cfg(target_arch = "x86_64")]
    {
        let save_idx = current;
        if save_idx < MAX_TASKS {
            let fs_base: u64;
            unsafe {
                core::arch::asm!(
                    "rdmsr",
                    "shl rdx, 32",
                    "or rax, rdx",
                    out("rax") fs_base,
                    out("rdx") _,
                    in("ecx") 0xC0000100u32,
                );            }
            TASK_FS_BASE[save_idx].store(fs_base, Ordering::Relaxed);
        }
    }

    // Mark the next task as running BEFORE __switch.
    CURRENT_RUNNING.store(next, Ordering::Relaxed);

    let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
    if switch_to_init {
        let init_sp_ptr = INIT_TASK_SP.as_ptr() as *const usize;
        unsafe {
            __switch(cur_ptr, init_sp_ptr);
        }
    } else {
        let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
        // Check if the next task has been properly saved by a Timer ISR context switch.
        // If TASK_SPS[next] still equals the initial value from add_clone_process,
        // the task's kernel stack TrapContext has likely been overwritten by SYSCALL handler
        // pushes. Switching to it would load corrupted data → kernel crash.
        // Fall back to init in this case.
        let nxt_sp = TASK_SPS[next].load(Ordering::Relaxed);
        let initial_sp = INITIAL_TASK_SP[next].load(Ordering::Relaxed);
        if initial_sp != 0 && nxt_sp == initial_sp {
            crate::klog!(DEBUG, "[EXIT] Task {} never saved, switching to init", next);
            // Mark the unsaved task as exited too
            {
                let mut sched = SCHEDULER.lock();
                if let Some(ref mut t) = sched.tasks[next] {
                    t.state = TaskState::Exited;
                }
            }
            let init_sp_ptr = INIT_TASK_SP.as_ptr() as *const usize;
            unsafe {
                __switch(cur_ptr, init_sp_ptr);
            }
        } else {
            unsafe {
                __switch(cur_ptr, nxt_ptr);
            }
        }
    }

    // Update TSS.RSP0 and SYSCALL_KSP for the new task
    #[cfg(target_arch = "x86_64")]
    {
        let proc_idx = if switch_to_init {
            INIT_PROC_IDX
        } else {
            next_proc_idx
        };
        if let Some(kernel_sp) = crate::process::get_kernel_sp(proc_idx) {
            crate::arch::idt::set_syscall_ksp(kernel_sp as u64);
            unsafe {
                crate::arch::gdt::set_kernel_rsp0_for_cpu(0, kernel_sp as u64);
            }
        }
        // Restore FS_BASE for the new task (critical for Go TLS / getg())
        let task_idx = CURRENT_RUNNING.load(Ordering::Relaxed);
        if task_idx < MAX_TASKS {
            let fs_base = TASK_FS_BASE[task_idx].load(Ordering::Relaxed);
            if fs_base != 0 {
                unsafe {
                    core::arch::asm!(
                        "wrmsr",
                        in("ecx") 0xC0000100u32,
                        in("edx") (fs_base >> 32) as u32,
                        in("eax") (fs_base & 0xFFFFFFFF) as u32,
                    );
                }
            }
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

/// Mark a scheduler task as Exited by its process table index.
/// Used by kill_clone_children to terminate clone child threads.
pub fn mark_task_exited_by_proc(proc_idx: usize) {
    let slot = PROC_TO_SLOT[proc_idx].load(Ordering::Relaxed);
    if slot >= MAX_TASKS {
        return;
    }
    let mut sched = SCHEDULER.lock();
    // Free the scheduler slot so it can be reused by new clone threads
    sched.tasks[slot] = None;
}

pub fn schedule_block() {
    let (switch_to_init, current, next, next_proc_idx) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        if current >= sched.count {
            return; // init has no TCB / nothing to block
        }
        if let Some(ref mut t) = sched.tasks[current] {
            t.state = TaskState::Blocked;
        }

        let mut next: usize = current;
        let start = LAST_SCHEDULED.load(Ordering::Relaxed) + 1;
        for i in 0..sched.count {
            let candidate = (start + i) % sched.count;
            if candidate == current {
                continue;
            }
            if let Some(ref t) = sched.tasks[candidate] {
                if t.state == TaskState::Ready {
                    next = candidate;
                    break;
                }
            }
        }

        if next == current {
            // No other Ready child — switch back to init to avoid busy-loop.
            // The blocked task will be re-scheduled when wake_task() is called.
            sched.current = INIT_SENTINEL;
            let n = SCHED_LOG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < SCHED_LOG_LIMIT {
                crate::console_println!("[sched] #{} block: slot{} -> init (no Ready)", n, current);
            }
            (true, current, 0usize, 0usize)
        } else {
            LAST_SCHEDULED.store(next, Ordering::Relaxed);
            if let Some(ref mut t) = sched.tasks[next] {
                t.state = TaskState::Running;
            }
            sched.current = next;
            let next_proc_idx = sched.task_to_process[next];
            crate::process::set_current_index(next_proc_idx);
            crate::process::set_current_page_table_root(crate::process::get_page_table_root(
                next_proc_idx,
            ));
            let n = SCHED_LOG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < SCHED_LOG_LIMIT {
                crate::console_println!("[sched] #{} block: slot{} -> slot{}", n, current, next);
            }
            (false, current, next, next_proc_idx)
        }
    };

    // Save FS_BASE for the current task
    #[cfg(target_arch = "x86_64")]
    {
        let msr_val = unsafe { crate::arch::idt::rdmsr(0xC0000100) };
        TASK_FS_BASE[current].store(msr_val, Ordering::Relaxed);
    }

    if switch_to_init {
        // Switch from blocked child to init
        let init_sp_ptr = INIT_TASK_SP.as_ptr() as *mut usize;
        unsafe {
            __switch(
                &TASK_SPS[current] as *const AtomicUsize as *mut usize,
                init_sp_ptr,
            );
        }
    } else {
        CURRENT_RUNNING.store(next, Ordering::Relaxed);
        let cur_ptr: *mut usize = &TASK_SPS[current] as *const AtomicUsize as *mut usize;
        let nxt_ptr: *const usize = &TASK_SPS[next] as *const AtomicUsize as *const usize;
        unsafe {
            __switch(cur_ptr, nxt_ptr);
        }
    }

    // After __switch returns — update per-task state
    #[cfg(target_arch = "x86_64")]
    {
        let cur = CURRENT_RUNNING.load(Ordering::Relaxed);
        let n = SCHED_LOG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < SCHED_LOG_LIMIT {
            crate::console_println!(
                "[sched] #{} block_resume: CURRENT_RUNNING={} (expected {})",
                n,
                cur,
                current
            );
        }
        if cur < MAX_TASKS {
            let kernel_sp = TASK_KSTACK[cur].load(Ordering::Relaxed);
            if kernel_sp != 0 {
                crate::arch::idt::set_syscall_ksp(kernel_sp as u64);
                unsafe {
                    crate::arch::gdt::set_kernel_rsp0_for_cpu(0, kernel_sp as u64);
                }
            }
        }
        let saved = TASK_FS_BASE[cur].load(Ordering::Relaxed);
        unsafe { crate::arch::idt::wrmsr(0xC0000100, saved) };
    }
}

pub fn wake_task(proc_idx: usize) {
    let mut sched = SCHEDULER.lock();
    for i in 0..sched.count {
        if sched.task_to_process[i] == proc_idx {
            if let Some(ref mut t) = sched.tasks[i] {
                if t.state == TaskState::Blocked {
                    t.state = TaskState::Ready;
                    let n = SCHED_LOG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if n < SCHED_LOG_LIMIT {
                        crate::console_println!(
                            "[sched] #{} wake: slot{} (proc{}) Blocked->Ready",
                            n,
                            i,
                            proc_idx
                        );
                    }
                }
            }
            return;
        }
    }
}

// ── Timer-based sleep queue ──────────────────────────────────────────
// Each entry: (task_slot_index, wake_tick)
// wake_tick = uptime_ms() value at which to wake the task.
// Called from timer ISR to check for expired sleeps.

use alloc::vec::Vec;

/// Global sleep queue protected by a spin lock.
static SLEEP_QUEUE: SpinLock<Vec<(usize, u64)>> = SpinLock::new(Vec::new());

/// Block the current task and schedule it to wake at `wake_tick` (uptime_ms).
/// Called from nanosleep / epoll_wait timeout.
pub fn sleep_until(wake_tick: u64) {
    let task_slot = CURRENT_RUNNING.load(Ordering::Relaxed);
    // task_slot == MAX_TASKS means init — init must never block
    let max_tasks = {
        let sched = SCHEDULER.lock();
        sched.count
    };
    if task_slot >= max_tasks {
        // Init: busy-wait instead (init must never block)
        while crate::arch::platform::uptime_ms() < wake_tick {
            core::hint::spin_loop();
        }
        return;
    }
    let now = crate::arch::platform::uptime_ms();
    // ALWAYS print — critical for diagnosing epoll_wait blocking
    crate::console_println!(
        "[sleep] slot={} now={} target={}",
        task_slot,
        now,
        wake_tick
    );
    if wake_tick <= now {
        return; // Already expired, no need to block
    }
    {
        let n = SCHED_LOG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < SCHED_LOG_LIMIT {
            crate::console_println!(
                "[sched] #{} sleep: slot{} now={} target={}",
                n,
                task_slot,
                now,
                wake_tick
            );
        }
        let mut q = SLEEP_QUEUE.lock();
        q.push((task_slot, wake_tick));
    }
    // Block the task (schedule_block switches to another Ready task)
    schedule_block();
}

/// Called from timer ISR every tick (~10ms).
/// Checks all sleeping tasks and wakes those whose deadline has passed.
/// Uses try_lock to avoid deadlock in ISR context.
pub fn tick_sleep_queue() {
    let now = crate::arch::platform::uptime_ms();
    let mut to_wake = Vec::new();
    {
        let mut q = match SLEEP_QUEUE.try_lock() {
            Some(guard) => guard,
            None => {
                static SLEEP_MISS: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let n = SLEEP_MISS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 5 {
                    crate::console_println!("[sleep] SLEEP_QUEUE try_lock failed (miss #{})", n);
                }
                return;
            }
        };
        let mut i = 0;
        while i < q.len() {
            if q[i].1 <= now {
                to_wake.push(q[i].0);
                q.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
    if to_wake.is_empty() {
        return;
    }
    let n = SCHED_LOG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if n < SCHED_LOG_LIMIT {
        crate::console_println!(
            "[sched] #{} tick_wake: now={} waking {} tasks",
            n,
            now,
            to_wake.len()
        );
    }
    let mut sched = match SCHEDULER.try_lock() {
        Some(guard) => guard,
        None => {
            static SCHED_MISS: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = SCHED_MISS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 5 {
                crate::console_println!("[sleep] SCHEDULER try_lock failed (miss #{})", n);
            }
            return;
        }
    };
    for task_slot in to_wake {
        if let Some(ref mut t) = sched.tasks[task_slot] {
            if t.state == TaskState::Blocked {
                t.state = TaskState::Ready;
            }
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

/// Add a user process to the scheduler.
///
/// On RISC-V: builds a TrapContext on the kernel stack with the RISC-V layout.
/// On x86_64: builds a TrapContext on the kernel stack with the x86_64 layout.
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

    #[cfg(target_arch = "riscv64")]
    {
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
        INITIAL_TASK_SP[tid].store(switch_sp, Ordering::Relaxed);
        PROC_TO_SLOT[process_idx].store(tid, Ordering::Relaxed);
    }

    #[cfg(target_arch = "x86_64")]
    {
        // x86_64 stack layout (from high to low):
        //   kernel_stack_top
        //     └─ TrapContext (ctx_size bytes)
        //        └─ __switch frame:
        //           +568: return address (→ first_task_shim)
        //           +560: rbp
        //           +552: rbx
        //           +544: r12
        //           +536: r13
        //           +528: r14
        //           +520: r15          ← orig_rsp points here
        //           +512: orig_rsp     ← saved original RSP for restore
        //           +0..+511: fxsave area (512 bytes)
        //           +0   ← switch_sp (TASK_SPS[tid])
        //
        // __switch restore: fxrstor → mov rsp,[rsp+512] (orig_rsp) →
        //                   pop r15..rbp → ret → first_task_shim
        let ctx_size = core::mem::size_of::<crate::arch::trap::TrapContext>();
        let switch_frame_size: usize = 8 * 8 + 512; // orig_rsp + 6 callee-saved + ret addr + fxsave = 576
        // Align switch_sp to 16 bytes so fxsave64/fxrstor64 doesn't #GP.
        // TrapContext sits above the switch frame, aligned to 8 bytes.
        let switch_sp = (kernel_stack_top - ctx_size - switch_frame_size) & !0xF;
        let trap_ctx_base = switch_sp + switch_frame_size;
        unsafe {
            core::ptr::write_bytes(switch_sp as *mut u8, 0, ctx_size + switch_frame_size);
            // Write a valid fxsave image: zeroed is fine but MXCSR must have
            // reserved bits clear (it does, since it's zero). However, some
            // emulators/QEMU may be stricter. Set MXCSR to the power-on default
            // (0x1F80) to be safe.
            let fxsave_ptr = switch_sp as *mut u8;
            // MXCSR is at fxsave offset 24 (32-bit field)
            let mxcsr_ptr = fxsave_ptr.add(24) as *mut u32;
            *mxcsr_ptr = 0x1F80; // Power-on default: all exceptions masked
            let sw = switch_sp as *mut usize;
            // orig_rsp: after __switch loads this, pop r15..rbp (48 bytes) then
            // ret pops the return address. So orig_rsp must point at the r15 slot.
            *sw.add(512 / 8) = switch_sp + 520; // orig_rsp → r15 slot
            // Return address at offset 568 (pop r15..rbp brings RSP to 568)
            *sw.add(568 / 8) = first_task_shim as *const () as usize; // ret addr
            // Build TrapContext
            let ctx = trap_ctx_base as *mut crate::arch::trap::TrapContext;
            (*ctx).rip = entry as u64;
            (*ctx).cs = crate::arch::gdt::USER_CODE_SEL.load(Ordering::Relaxed) as u64;
            (*ctx).rflags = 0x202;
            (*ctx).rsp = user_stack_top as u64;
            (*ctx).ss = crate::arch::gdt::USER_DATA_SEL.load(Ordering::Relaxed) as u64;
            (*ctx).kernel_sp = kernel_stack_top as u64;
            (*ctx).user_cr3 = user_satp as u64;
            (*ctx).trap_from_user = 1;
            // Write magic values for debug
            (*ctx).r15 = 0xDEADBEEF_CAFEBABEu64;
            (*ctx).r14 = 0x12345678_87654321u64;
        }
        TASK_SPS[tid].store(switch_sp, Ordering::Relaxed);
        INITIAL_TASK_SP[tid].store(switch_sp, Ordering::Relaxed);
        PROC_TO_SLOT[process_idx].store(tid, Ordering::Relaxed);
        #[cfg(target_arch = "x86_64")]
        TASK_KSTACK[tid].store(kernel_stack_top as u64, Ordering::Relaxed);
    }

    let tcb = TaskControlBlock::new(tid);
    sched.tasks[tid] = Some(tcb);
    sched.task_to_process[tid] = process_idx;
    if tid >= sched.count {
        sched.count = tid + 1;
    }
    // Mark the new task as Ready (NOT Running). The child will be dispatched
    // when init calls schedule() via sys_waitpid or sys_read.
    if let Some(ref mut t) = sched.tasks[tid] {
        t.state = TaskState::Ready;
    }
    // Do NOT set sched.current = tid here! Init is still running.
    // sched.current remains INIT_SENTINEL so that schedule() takes the
    // switch_from_init path and correctly saves init's RSP to INIT_TASK_SP.
    Some(tid)
}

/// Clone-specific first entry shim for x86_64.
///
/// When a clone child is first scheduled, __switch returns to this function.
/// It reads the TLS value stored after the TrapContext on the kernel stack,
/// sets IA32_FS_BASE if non-zero, then falls through to trap_return_user.
///
/// Stack layout on entry (rsp points to TrapContext base):
///   [rsp + 0x00 .. 0xB7] = TrapContext (184 bytes)
///   [rsp + 0xB8]           = TLS value (8 bytes, 0 = no TLS)
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn clone_first_shim() -> ! {
    unsafe {
        core::arch::naked_asm!(
            // RSP = TrapContext base (after __switch: pop r15..rbp + ret)
            // Self-contained: does NOT use trap_return_user.
            //
            // TrapContext layout (15 pop reveals this):
            //   +0x00: rip        +0x08: cs      +0x10: rflags
            //   +0x18: user_rsp   +0x20: ss
            //   +0x28: kernel_sp  +0x30: user_cr3  +0x38: trap_from_user
            "cli",

            // 0. Disable interrupts — we're in kernel mode manipulating TrapContext.
            // Timer ISR would corrupt our stack if it fires during this sequence.
            "cli",

            // 1. Set TSS.RSP0 and SYSCALL_KSP from r14 (kernel_stack_top)
            // r14 was loaded by __switch RESTORE from per-task switch frame slot 528
            // (no longer uses global PENDING_RSP0 — avoids race with multiple clones)
            "mov rax, r14",
            "test rax, rax",
            "jz 1f",
            "mov rcx, [rip + {tss_addr}]",
            "test rcx, rcx",
            "jz 1f",
            "mov [rcx], rax",
            // Also set SYSCALL_KSP to the same value
            "mov [rip + {syscall_ksp}], rax",
            "1:",

            // 2. Set FS_BASE from r15 (loaded by __switch RESTORE)
            "mov rax, r15",
            "cmp rax, 0",
            "je 2f",
            "mov ecx, 0xC0000100",
            "mov rdx, rax",
            "shr rdx, 32",
            "wrmsr",
            "2:",

            // 3. Pop 15 GP regs from TrapContext
            "pop rax",
            "pop rbx",
            "pop rcx",
            "pop rdx",
            "pop rbp",
            "pop rsi",
            "pop rdi",
            "pop r8",
            "pop r9",
            "pop r10",
            "pop r11",
            "pop r12",
            "pop r13",
            "pop r14",
            "pop r15",

            // 4. Now RSP points to: rip, cs, rflags, rsp, ss, kernel_sp, user_cr3, trap_from_user
            //    Read iretq frame and CR3 BEFORE switching page tables.
            //    Use registers that don't conflict with 15 pop values.
            "mov r14, [rsp + 0x00]",  // rip
            "mov r15, [rsp + 0x08]",  // cs
            "mov rbx, [rsp + 0x10]", // rflags
            "mov rdx, [rsp + 0x18]", // user rsp
            "mov rdi, [rsp + 0x20]", // ss
            // Use r10 for user_cr3 — Go needs R8=mp, R9=gp, R12=fn from clone SYSCALL
            "mov r10, [rsp + 0x30]", // user_cr3 (MUST NOT use r8 — Go's mp pointer!)

            // 5. Switch CR3 if user_cr3 != 0
            "cmp r10, 0",
            "je 3f",
            "mov rax, r10",
            "mov cr3, rax",
            "3:",

            // 6. Restore rax = 0 (child return value from clone)
            "xor rax, rax",

            // 7. Validate RIP before iretq — if < 0x400000, TrapContext is corrupted
            "cmp r14, 0x400000",
            "jae 4f",
            // Invalid RIP! Print diagnostic and halt
            "mov rsi, r14",       // bad RIP value
            "mov rdi, rsp",       // current RSP for context
            "mov rax, 0xB8000",   // VGA buffer
            "mov byte ptr [rax], 'C'",
            "mov byte ptr [rax+2], 'R'",
            "mov byte ptr [rax+4], 'P'",
            "mov byte ptr [rax+6], 'F'",  // CLONE RIP FAIL
            "cli",
            "hlt",
            "4:",

            // 7. Restore user RSP and iretq
            "mov rsp, rdx",
            "push rdi",    // ss
            "push rdx",    // rsp
            "push rbx",    // rflags
            "push r15",    // cs
            "push r14",    // rip
            "iretq",

            tss_addr = sym crate::arch::gdt::TSS_RSP0_ADDR,
            syscall_ksp = sym crate::arch::idt::SYSCALL_KSP,
        );
    }
}

/// Add a clone child task to the scheduler.
///
/// Unlike `add_user_process` which creates a fresh TrapContext for a new process,
/// this copies the parent's register state so the child resumes at the exact
/// point after the clone syscall, with rax=0 and a new user stack.
///
/// # Arguments
/// - `parent_ctx`: Parent's TrapContext (register state at clone time)
/// - `new_user_sp`: New user stack pointer for the child (from clone flags.stack)
/// - `kernel_stack_top`: Child's kernel stack top (freshly allocated)
/// - `user_cr3`: Page table physical address (shared for CLONE_VM)
/// - `process_idx`: Process table index for the child
/// - `tls`: TLS address for CLONE_SETTLS (0 = no TLS)
///
/// Returns the task slot ID (tid), or None if no slot available.
#[cfg(target_arch = "x86_64")]
pub fn add_clone_process(
    parent_ctx: &crate::arch::trap::TrapContext,
    new_user_sp: usize,
    kernel_stack_top: usize,
    user_cr3: usize,
    process_idx: usize,
    tls: usize,
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
    let tls_storage_size: usize = 0; // No TLS storage - same layout as add_user_process
    // __switch frame layout (same as add_user_process):
    //   [0..511]   fxsave area (512 bytes)
    //   [512..519] orig_rsp (points to r15 slot after fxrstor)
    //   [520..567] r15, r14, r13, r12, rbx, rbp (6 callee-saved × 8)
    //   [568..575] ret addr
    // Total: 576 bytes
    let switch_frame_size: usize = 8 * 8 + 512; // orig_rsp + 6 callee-saved + ret addr + fxsave = 576
    let total_size = ctx_size + tls_storage_size + switch_frame_size;
    // Layout (high → low):
    //   kernel_stack_top
    //   TrapContext (ctx_size)
    //   TLS storage (8 bytes)
    //   __switch frame (576 bytes)
    // Align switch_sp to 16 bytes for fxsave64/fxrstor64.
    let switch_sp = (kernel_stack_top - ctx_size - tls_storage_size - switch_frame_size) & !0xF;
    let tls_base = switch_sp + switch_frame_size;
    let trap_ctx_base = tls_base + tls_storage_size;

    unsafe {
        core::ptr::write_bytes(switch_sp as *mut u8, 0, total_size);

        // Set MXCSR to power-on default (same as add_user_process)
        let fxsave_ptr = switch_sp as *mut u8;
        let mxcsr_ptr = fxsave_ptr.add(24) as *mut u32;
        *mxcsr_ptr = 0x1F80; // Power-on default: all exceptions masked

        let sw = switch_sp as *mut usize;
        // orig_rsp: points to r15 slot so pop r15..rbp + ret works correctly
        *sw.add(512 / 8) = switch_sp + 520; // orig_rsp → r15 slot
        *sw.add(520 / 8) = tls; // r15 = TLS
        // Return address at offset 568 (pop r15..rbp brings RSP here)
        *sw.add(568 / 8) = clone_first_shim as *const () as usize; // ret addr → clone_first_shim (sets TLS via wrmsr)

        // r14 = kernel_stack_top (for clone_first_shim to set RSP0 + SYSCALL_KSP)
        *sw.add(528 / 8) = kernel_stack_top;

        // Build TrapContext as a copy of parent's, with modifications for child
        let ctx = trap_ctx_base as *mut crate::arch::trap::TrapContext;
        *ctx = parent_ctx.clone();

        // Debug: validate parent's TrapContext.rip
        let parent_rip = parent_ctx.rip;
        if parent_rip < 0x400000 {
            crate::console_println!(
                "[clone] WARNING: parent_rip={:#x} is below 0x400000! TrapContext may be corrupted!",
                parent_rip
            );
            // Dump first 8 words of parent's TrapContext
            let p = parent_ctx as *const crate::arch::trap::TrapContext as *const u64;
            for i in 0..8 {
                crate::console_println!("  parent_ctx[{}] = {:#x}", i, unsafe { *p.add(i) });
            }
        }

        // Modifications for clone child:
        (*ctx).rax = 0; // Child returns 0 from clone
        (*ctx).rsp = new_user_sp as u64; // Use new user stack
        (*ctx).kernel_sp = kernel_stack_top as u64;
        (*ctx).user_cr3 = user_cr3 as u64; // Set for CR3 switch on first entry
        (*ctx).trap_from_user = 1;
        // Set TrapContext.r15 = TLS value. clone_first_shim's pop r15 will
        // load this into r15 register (overwriting __switch's r15). Both paths
        // need to agree on the TLS value.
        (*ctx).r15 = tls as u64;
        // Debug: print child's TrapContext.rip to verify
        crate::console_println!(
            "[clone] child TrapContext: rip={:#x} rsp={:#x} r8={:#x} r9={:#x} r12={:#x}",
            (*ctx).rip,
            (*ctx).rsp,
            (*ctx).r8,
            (*ctx).r9,
            (*ctx).r12
        );
    }

    TASK_SPS[tid].store(switch_sp, Ordering::Relaxed);
    INITIAL_TASK_SP[tid].store(switch_sp, Ordering::Relaxed);
    PROC_TO_SLOT[process_idx].store(tid, Ordering::Relaxed);
    #[cfg(target_arch = "x86_64")]
    {
        TASK_KSTACK[tid].store(kernel_stack_top as u64, Ordering::Relaxed);
        PENDING_RSP0.store(kernel_stack_top as u64, Ordering::Relaxed);
    }

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

/// Get the kernel stack pointer of the currently running task.
/// Used by the syscall fast entry path (SYSCALL instruction) to set up its kernel stack.
#[cfg(target_arch = "x86_64")]
/// Get per-task FS_BASE (lock-free, safe in ISR context).
#[cfg(target_arch = "x86_64")]
pub fn get_task_fs_base(task_idx: usize) -> u64 {
    if task_idx < MAX_TASKS {
        TASK_FS_BASE[task_idx].load(core::sync::atomic::Ordering::Relaxed)
    } else {
        0
    }
}

/// Set per-task FS_BASE (called from arch_prctl and linux_clone).
#[cfg(target_arch = "x86_64")]
pub fn set_task_fs_base(task_idx: usize, val: u64) {
    if task_idx < MAX_TASKS {
        TASK_FS_BASE[task_idx].store(val, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Get the current scheduler slot (may differ from process table index).
#[cfg(target_arch = "x86_64")]
pub fn current_sched_slot() -> usize {
    let proc_idx = crate::process::current_index();
    PROC_TO_SLOT[proc_idx].load(Ordering::Relaxed)
}

pub fn current_kernel_sp() -> usize {
    let sched = SCHEDULER.lock();
    let current = sched.current;
    if current == INIT_SENTINEL {
        // Init is running — return INIT_TASK_SP
        INIT_TASK_SP.load(Ordering::Relaxed)
    } else if current < sched.count {
        // A child task is running — return its kernel_sp from the process table
        let proc_idx = sched.task_to_process[current];
        crate::process::get_kernel_sp(proc_idx).unwrap_or(0)
    } else {
        0
    }
}

pub fn current_brk() -> usize {
    crate::process::current_brk()
}

pub fn set_current_brk(addr: usize) {
    crate::process::set_current_brk(addr);
}

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

    // After __switch returns (possibly on a different task's stack),
    // update TSS.RSP0 so Ring 3 → Ring 0 interrupts use the correct kernel stack.
    #[cfg(target_arch = "x86_64")]
    {
        let proc_idx = crate::process::current_index();
        if let Some(kernel_sp) = crate::process::get_kernel_sp(proc_idx) {
            unsafe {
                crate::arch::gdt::set_kernel_rsp0(kernel_sp as u64);
            }
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

    // Update TSS.RSP0 for the new task
    #[cfg(target_arch = "x86_64")]
    {
        let proc_idx = crate::process::current_index();
        if let Some(kernel_sp) = crate::process::get_kernel_sp(proc_idx) {
            unsafe {
                crate::arch::gdt::set_kernel_rsp0(kernel_sp as u64);
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
        //           +520: r15
        //           +0..+519: fxsave area (512 bytes)
        //           +0   ← switch_sp (TASK_SPS[tid])
        //
        // __switch: fxrstor → pop r15..rbp → ret → first_task_shim
        let ctx_size = core::mem::size_of::<crate::arch::trap::TrapContext>();
        let switch_frame_size: usize = 7 * 8 + 512; // 6 callee-saved + ret addr + fxsave
        let trap_ctx_base = kernel_stack_top - ctx_size;
        let switch_sp = trap_ctx_base - switch_frame_size;
        unsafe {
            core::ptr::write_bytes(switch_sp as *mut u8, 0, ctx_size + switch_frame_size);
            // __switch frame: fxsave area at bottom (512 bytes, zeroed above),
            // then callee-saved (also zeroed), then ret addr at top
            let sw = switch_sp as *mut usize;
            *sw.add(512 / 8 + 6) = first_task_shim as *const () as usize; // ret addr
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
        }
        TASK_SPS[tid].store(switch_sp, Ordering::Relaxed);
    }

    let tcb = TaskControlBlock::new(tid);
    sched.tasks[tid] = Some(tcb);
    sched.task_to_process[tid] = process_idx;
    if tid >= sched.count {
        sched.count = tid + 1;
    }
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
            // Read TLS value stored right after TrapContext
            "mov rax, [rsp + 0xB8]",
            "cmp rax, 0",
            "je 2f",
            // Set IA32_FS_BASE MSR (0xC0000100)
            "mov rcx, 0xC0000100",
            "wrmsr",
            "2:",
            // Fall through to trap_return_user
            "mov rdi, rsp",
            "jmp {handler}",
            handler = sym trap_return_user,
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
    let tls_storage_size: usize = 8; // 8 bytes for TLS after TrapContext
    let switch_frame_size: usize = 7 * 8 + 512; // 6 callee-saved + ret addr + fxsave
    let total_size = ctx_size + tls_storage_size + switch_frame_size;
    let tls_base = kernel_stack_top - ctx_size - tls_storage_size;
    let trap_ctx_base = kernel_stack_top - ctx_size;
    let switch_sp = tls_base - switch_frame_size;

    unsafe {
        core::ptr::write_bytes(switch_sp as *mut u8, 0, total_size);

        // __switch frame: fxsave area at bottom (512 bytes, zeroed above),
        // then callee-saved (also zeroed), then ret addr at top
        let sw = switch_sp as *mut usize;
        *sw.add(512 / 8 + 6) = clone_first_shim as *const () as usize; // ret addr → clone_first_shim

        // Store TLS value after TrapContext
        let tls_ptr = tls_base as *mut usize;
        *tls_ptr = tls;

        // Build TrapContext as a copy of parent's, with modifications for child
        let ctx = trap_ctx_base as *mut crate::arch::trap::TrapContext;
        *ctx = parent_ctx.clone();
        // Modifications for clone child:
        (*ctx).rax = 0;                       // Child returns 0 from clone
        (*ctx).rsp = new_user_sp as u64;      // Use new user stack
        (*ctx).kernel_sp = kernel_stack_top as u64;
        (*ctx).user_cr3 = user_cr3 as u64;    // Set for first entry (switches CR3)
        (*ctx).trap_from_user = 1;
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

/// Get the kernel stack pointer of the currently running task.
/// Used by the syscall fast entry path (SYSCALL instruction) to set up its kernel stack.
#[cfg(target_arch = "x86_64")]
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

//! Typed x86_64 user-return architecture state.
//!
//! When the kernel returns to user mode (via iretq, sysret, or trap_return_user),
//! it must restore all per-task hardware state:
//! - User CR3 (page table root)
//! - Kernel RSP0 (TSS stack for Ring 3→Ring 0 transitions)
//! - FS_BASE MSR (thread-local storage pointer)
//!
//! The key invariant: `FsBase::ZERO` is a valid value and must be written to the
//! MSR explicitly. The restore APIs never skip writing because the stored value
//! is zero. This prevents the class of bugs where Go's `%fs:-8` dereference
//! faults after a syscall return that left FS_BASE stale.

/// Thread-local storage base (IA32_FS_BASE MSR 0xC0000100).
///
/// `FsBase::ZERO` means "no TLS" — a valid state that must be written to hardware.
/// Do NOT use `Option<FsBase>` where None means "no TLS"; use `FsBase::ZERO`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FsBase(u64);

impl FsBase {
    /// No TLS — the task's FS_BASE is zero.
    pub const ZERO: Self = Self(0);

    /// Create from a raw MSR value.
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw MSR value.
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Whether this represents "no TLS".
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

/// User page table root for CR3 loading.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UserCr3Value(u64);

impl UserCr3Value {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Kernel stack pointer for TSS.RSP0 (Ring 3→Ring 0 interrupt stack).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KernelRsp0(u64);

impl KernelRsp0 {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Complete state needed to return to user mode.
///
/// Every user-return path must restore all three values.
/// Partial restoration (e.g., only CR3 but not FS_BASE) is a bug.
#[derive(Clone, Copy, Debug)]
pub struct UserReturnState {
    pub user_cr3: Option<UserCr3Value>,
    pub kernel_rsp0: Option<KernelRsp0>,
    pub fs_base: FsBase,
}

impl UserReturnState {
    /// Create with all fields unset (identity kernel CR3, no RSP0 change, FS_BASE=0).
    pub const fn default_for_test() -> Self {
        Self {
            user_cr3: None,
            kernel_rsp0: None,
            fs_base: FsBase::ZERO,
        }
    }
}

// ─── Restore functions ────────────────────────────────────────────────

/// Write FS_BASE to the MSR. Always writes, even if value is zero.
pub fn restore_fs_base(fs_base: FsBase) {
    unsafe {
        crate::arch::idt::wrmsr(0xC0000100, fs_base.raw());
    }
}

/// Read the current FS_BASE from the MSR.
pub fn read_fs_base() -> FsBase {
    FsBase::new(unsafe { crate::arch::idt::rdmsr(0xC0000100) })
}

/// Restore the full user-return state for a task about to return to Ring 3.
///
/// This must be called on every user-return path:
/// - `iretq` from timer interrupt / trap handler
/// - `sysret` from Linux compat syscall fast return
/// - `trap_return_user` first-entry assembly
pub fn restore_for_user_return(state: UserReturnState) {
    if let Some(cr3) = state.user_cr3 {
        // CR3 switch is handled by trap_return_user / specific return paths.
        // We record it but the actual CR3 write is in assembly or specific helpers.
        // For now, we don't write CR3 here to avoid double-switching.
        let _ = cr3;
    }
    if let Some(rsp0) = state.kernel_rsp0 {
        unsafe {
            crate::arch::gdt::set_kernel_rsp0_for_cpu(0, rsp0.raw());
        }
    }
    // ALWAYS write FS_BASE, even if zero.
    restore_fs_base(state.fs_base);
}

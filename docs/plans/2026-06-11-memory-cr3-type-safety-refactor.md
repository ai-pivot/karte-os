# Memory and CR3 Type-Safety Refactor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make x86_64 memory management, CR3 switching, user memory access, and page-fault repair hard to misuse by encoding the critical invariants in Rust types instead of comments and ad hoc checks.

**Architecture:** Introduce a small typed memory core: address newtypes, frame ownership types, page-table level types, per-process address-space ownership, and RAII CR3 guards. Then migrate syscall, PF, mmap, ELF loading, and driver buffer paths so raw `usize` addresses and manual CR3 switching are confined to a few audited modules.

**Tech Stack:** Rust 2024 `no_std`, `alloc`, const generics/phantom types, sealed traits, RAII guards, `#[must_use]`, architecture-gated modules, QEMU integration tests.

---

## Problem Summary

The recent `xbot-cli-static` failures exposed recurring classes of bugs:

- Kernel buffers were accessed while the hardware CR3 pointed at a user page table.
- User pointers, kernel pointers, physical addresses, and virtual addresses were all plain `usize`.
- `translate_user()` could report supervisor identity huge pages as if they were real user mappings.
- `mprotect_user()` could descend through a 2MB huge page as if it were a lower-level page table.
- Page-fault repair mixed three different concepts: ELF file mappings, anonymous mappings, and copied identity mappings.
- VMA state is global even though VMAs are address-space state.
- x86_64 user-return architecture state was incomplete: CR3 returned to the user page table, but Linux `SYSCALL` return could leave `FS_BASE` stale or zero. Go immediately reads its g pointer through `%fs:-8`; after `sched_getaffinity(204)`, this produced `PF UNHANDLED addr=0x0 rip=0x44fa78`.
- Scheduler arch-state restore treated `FS_BASE == 0` as "do nothing". Zero is a valid state and must be written explicitly when switching to tasks that do not use TLS.
- Logs print many symptoms but not enough typed state at the failing boundary: active CR3, address-space root, PTE chain, VMA owner, leaf kind, and access kind.

The target state is: if code has a `KernelVirtAddr`, it cannot be passed to a function requiring `UserVirtAddr`; if code wants to walk page tables, it must handle `HugePage` explicitly; if code wants to copy user memory, it must hold a `UserAccess` capability; if code wants to switch CR3, the switch is restored by `Drop`; if code returns to user mode, it must restore the full typed `UserReturnState`, not just CR3.

## Non-Goals

- Do not redesign the whole scheduler.
- Do not add copy-on-write.
- Do not implement Linux-compatible signal delivery beyond the existing stubs.
- Do not optimize page-table operations before making them type-safe and testable.
- Do not hide diagnostics behind rate limits. Runtime diagnostic logs stay unconditional; filtering happens offline.

---

## Target Invariants

1. Raw address `usize` is only allowed at architecture boundaries: assembly entry, CR3 read/write, PTE encoding/decoding, MMIO port/MMIO volatile access, and syscall ABI decode.
2. All process memory operations go through an `AddressSpace` value, not global VMA state.
3. Page-table walking returns a typed result:
   - `NotMapped`
   - `Mapped4K`
   - `MappedHuge { level, size }`
   - `Invalid`
4. `mprotect`, `munmap`, and PF lazy allocation must never silently treat a huge page as a lower-level page table.
5. User memory copy APIs can only be called with a user virtual pointer and an address-space root.
6. Kernel allocations and kernel buffers are always accessed under kernel CR3.
7. Any function that may switch CR3 returns with the original CR3 restored, including early returns.
8. ELF PT_LOAD mappings are file-backed private user mappings, never supervisor identity mappings.
9. Anonymous `mmap` mappings are zero-filled private user mappings, never identity mappings.
10. User-return paths restore all per-task x86_64 state: user CR3, kernel RSP0/SYSCALL stack, and `FS_BASE`.
11. `FS_BASE == 0` is a valid value and must be restored explicitly; no restore API may use zero as "leave unchanged".
12. Linux `SYSCALL` fast return must not rely on the hardware MSR value that happened to survive kernel execution.
13. Logs identify the failing typed boundary, not just the symptom.

---

## Task 1: Add Regression Tests for Current Failure Modes

**Files:**
- Modify: `kernel/src/mm/vmm.rs`
- Modify: `kernel/src/syscall/mod.rs`
- Modify: `kernel/src/process/mod.rs`
- Modify: `kernel/src/arch/x86_64/idt.rs`
- Modify: `kernel/src/arch/x86_64/test.rs`
- Modify: `kernel/src/sched/mod.rs`
- Test command: `make test-x86`

**Step 1: Add a VMM test for mprotect on copied huge identity mappings**

Add a `#[cfg(feature = "test_mode")]` test in `kernel/src/mm/vmm.rs`:

```rust
crate::test::run_test("vmm_mprotect_does_not_descend_into_huge_page", || {
    let root = create_user_page_table();
    identity_map_2mb(root, 0, 4 * 1024 * 1024, PTEFlags::KRW);

    let target = 0x401000usize;
    let before = translate_user(root, target);
    let changed = mprotect_user(root, target, PTEFlags::UR);
    let after = translate_user(root, target);

    !changed && before == after
});
```

**Step 2: Add a VMM test for splitting a 2MB huge page**

Add:

```rust
crate::test::run_test("vmm_map_split_2mb_stops_at_pt_level", || {
    let root = create_user_page_table();
    identity_map_2mb(root, 0, 4 * 1024 * 1024, PTEFlags::KRW);

    let frame = match crate::mm::pmm::alloc_frame() {
        Some(f) => f,
        None => return false,
    };
    map_user(root, 0x401000, frame, PTEFlags::UR);

    translate_user(root, 0x401000) == Some(frame)
});
```

**Step 3: Add a process test for exit-code visibility**

Add a tiny test helper in `kernel/src/process/mod.rs` guarded by `test_mode`:

```rust
crate::test::run_test("process_set_exit_code_by_index_marks_exited", || {
    let proc = Process::test_dummy(0xdead_0000);
    let idx = match add_process(proc) {
        Some(i) => i,
        None => return false,
    };

    set_exit_code_by_index(idx, 7);
    let observed = get_exit_code(idx);
    free_process_slot(idx);

    observed == Some(7)
});
```

If no test constructor exists, create a minimal `Process::test_dummy(page_table_root)` under `#[cfg(feature = "test_mode")]`.

**Step 4: Add a SYSCALL return FS_BASE regression test**

Add an x86_64 test in `kernel/src/arch/x86_64/test.rs`:

```rust
crate::test::run_test("x86_64 syscall_fs_restore", || {
    let slot = crate::sched::current_sched_slot();
    let orig_msr = unsafe { crate::arch::idt::rdmsr(0xC0000100) };
    let orig_task = crate::sched::get_task_fs_base(slot);
    let val = 0x4830_8a8u64;

    crate::sched::set_task_fs_base(slot, val);
    unsafe { crate::arch::idt::wrmsr(0xC0000100, 0) };
    crate::arch::idt::restore_current_task_fs_base_for_syscall_return();
    let restored = unsafe { crate::arch::idt::rdmsr(0xC0000100) };

    unsafe { crate::arch::idt::wrmsr(0xC0000100, orig_msr) };
    crate::sched::set_task_fs_base(slot, orig_task);

    restored == val
});
```

This test captures the xbot failure mode where Go successfully set TLS via `arch_prctl(ARCH_SET_FS)`, then a normal Linux syscall returned with `FS_BASE` lost before Go executed `mov %fs:-8`.

**Step 5: Add a scheduler zero-FS restore regression test**

Add a focused x86_64 test for the opposite leak:

```rust
crate::test::run_test("x86_64 restore_zero_fs_base", || {
    let slot = crate::sched::current_sched_slot();
    let orig_msr = unsafe { crate::arch::idt::rdmsr(0xC0000100) };
    let orig_task = crate::sched::get_task_fs_base(slot);

    unsafe { crate::arch::idt::wrmsr(0xC0000100, 0xdead_beef) };
    crate::sched::set_task_fs_base(slot, 0);
    crate::sched::restore_task_arch_state_for_test(slot);
    let restored = unsafe { crate::arch::idt::rdmsr(0xC0000100) };

    unsafe { crate::arch::idt::wrmsr(0xC0000100, orig_msr) };
    crate::sched::set_task_fs_base(slot, orig_task);

    restored == 0
});
```

If `restore_task_arch_state()` is private, expose a `#[cfg(feature = "test_mode")]` wrapper only for this test. The invariant is that zero must be written, not skipped.

**Step 6: Run tests and confirm current baseline**

Run: `make test-x86`

Expected before deeper refactor: all existing x86 tests should still pass except any already documented unrelated PMM layout test.

**Step 7: Commit**

```bash
git add kernel/src/mm/vmm.rs kernel/src/process/mod.rs kernel/src/syscall/mod.rs kernel/src/arch/x86_64/idt.rs kernel/src/arch/x86_64/test.rs kernel/src/sched/mod.rs
git commit -m "test(x86_64): cover memory and user-return regressions"
```

---

## Task 2: Introduce Typed Address Newtypes

**Files:**
- Create: `kernel/src/mm/addr.rs`
- Modify: `kernel/src/mm/mod.rs`
- Modify: `kernel/src/mm/vmm.rs`
- Modify: `kernel/src/process/mod.rs`

**Step 1: Add address types**

Create `kernel/src/mm/addr.rs`:

```rust
use core::fmt;
use core::marker::PhantomData;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Phys;
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Virt;
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct User;
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Kernel;

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Addr<Kind, Space = ()> {
    raw: usize,
    _kind: PhantomData<(Kind, Space)>,
}

pub type PhysAddr = Addr<Phys>;
pub type VirtAddr = Addr<Virt>;
pub type UserVirtAddr = Addr<Virt, User>;
pub type KernelVirtAddr = Addr<Virt, Kernel>;

impl<K, S> Addr<K, S> {
    pub const fn new_unchecked(raw: usize) -> Self {
        Self { raw, _kind: PhantomData }
    }

    pub const fn as_usize(self) -> usize {
        self.raw
    }

    pub const fn is_page_aligned(self) -> bool {
        self.raw & 0xfff == 0
    }

    pub const fn page_align_down(self) -> Self {
        Self::new_unchecked(self.raw & !0xfff)
    }
}

impl<K, S> fmt::Debug for Addr<K, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.raw)
    }
}
```

**Step 2: Add checked constructors for user and kernel virtual addresses**

Add:

```rust
impl UserVirtAddr {
    pub fn try_new(raw: usize) -> Option<Self> {
        if raw < crate::process::USER_MMAP_LIMIT {
            Some(Self::new_unchecked(raw))
        } else {
            None
        }
    }
}

impl KernelVirtAddr {
    pub fn try_new(raw: usize) -> Option<Self> {
        if raw >= crate::process::USER_MMAP_LIMIT {
            Some(Self::new_unchecked(raw))
        } else {
            None
        }
    }
}
```

Adjust canonical ranges for x86_64 once the address layout is formalized.

**Step 3: Re-export from `mm/mod.rs`**

```rust
pub mod addr;
```

**Step 4: Convert low-risk VMM helpers**

Convert signatures first where callsites are few:

```rust
pub fn page_offset(addr: UserVirtAddr) -> usize;
pub fn vpn(addr: UserVirtAddr, level: PageLevel) -> usize;
```

Do not convert all callsites in this task.

**Step 5: Run focused build**

Run: `make build-x86`

Expected: release kernel and ISO build successfully.

**Step 6: Commit**

```bash
git add kernel/src/mm/addr.rs kernel/src/mm/mod.rs kernel/src/mm/vmm.rs kernel/src/process/mod.rs
git commit -m "refactor(mm): add typed address wrappers"
```

---

## Task 3: Introduce Frame Ownership Types

**Files:**
- Create: `kernel/src/mm/frame.rs`
- Modify: `kernel/src/mm/mod.rs`
- Modify: `kernel/src/mm/pmm.rs`
- Modify: `kernel/src/mm/vmm.rs`

**Step 1: Define owned frame wrappers**

Create:

```rust
use crate::mm::addr::PhysAddr;

#[must_use]
pub struct OwnedFrame {
    addr: PhysAddr,
}

#[must_use]
pub struct PageTableFrame {
    addr: PhysAddr,
}

#[derive(Clone, Copy)]
pub struct BorrowedFrame {
    addr: PhysAddr,
}

impl OwnedFrame {
    pub fn addr(&self) -> PhysAddr {
        self.addr
    }

    pub fn into_raw(self) -> PhysAddr {
        let addr = self.addr;
        core::mem::forget(self);
        addr
    }
}

impl Drop for OwnedFrame {
    fn drop(&mut self) {
        crate::mm::pmm::dealloc_frame(self.addr.as_usize());
    }
}
```

Use `Drop` only after auditing callsites for ownership. If any callsite stores frames in page tables, it must use `into_raw()` explicitly.

**Step 2: Add allocation APIs without removing old ones yet**

In `pmm.rs`:

```rust
pub fn alloc_owned_frame() -> Option<crate::mm::frame::OwnedFrame>;
pub fn alloc_page_table_frame() -> Option<crate::mm::frame::PageTableFrame>;
```

Keep `alloc_frame()` temporarily for migration.

**Step 3: Convert `PageTable::zeroed()`**

Make page-table allocation explicit:

```rust
pub fn zeroed_frame() -> PageTableFrame;
pub unsafe fn from_frame_mut(frame: PageTableFrame) -> &'static mut PageTable;
```

Document the only unsafe reason: interpreting a zeroed 4KB physical frame as a page table.

**Step 4: Add tests for ownership**

Add tests that allocate and drop an `OwnedFrame`, then reallocate it, verifying PMM state does not leak.

**Step 5: Run tests**

Run: `make test-x86`

Expected: no new failures.

**Step 6: Commit**

```bash
git add kernel/src/mm/frame.rs kernel/src/mm/mod.rs kernel/src/mm/pmm.rs kernel/src/mm/vmm.rs
git commit -m "refactor(mm): add frame ownership types"
```

---

## Task 4: Encode Page-Table Levels in Types

**Files:**
- Create: `kernel/src/mm/page_table.rs`
- Modify: `kernel/src/mm/vmm.rs`
- Modify: `kernel/src/mm/mod.rs`

**Step 1: Define page-table level markers**

```rust
pub trait Level {
    const INDEX: usize;
}

pub enum L4 {}
pub enum L3 {}
pub enum L2 {}
pub enum L1 {}

impl Level for L4 { const INDEX: usize = 3; }
impl Level for L3 { const INDEX: usize = 2; }
impl Level for L2 { const INDEX: usize = 1; }
impl Level for L1 { const INDEX: usize = 0; }
```

**Step 2: Define typed entries**

```rust
pub enum EntryKind {
    Table,
    Leaf4K,
    LeafHuge,
    NotPresent,
}

pub struct PageEntry<L: Level> {
    raw: crate::mm::vmm::PTE,
    _level: core::marker::PhantomData<L>,
}
```

**Step 3: Make huge-page handling explicit**

Add:

```rust
pub enum WalkResult {
    NotMapped,
    Mapped4K { frame: PhysAddr, flags: PtePerms },
    MappedHuge { frame: PhysAddr, level: usize, size: usize, flags: PtePerms },
    Invalid,
}
```

**Step 4: Replace `translate_user()` internals**

Keep the public function temporarily, but implement it through:

```rust
pub fn walk_mapping(root: &mut PageTable, addr: UserVirtAddr) -> WalkResult;
```

Then make `translate_user()` return `Some` only for `Mapped4K` and explicitly documented huge-page cases.

**Step 5: Run regression tests**

Run: `make test-x86`

Expected: VMM huge-page tests pass.

**Step 6: Commit**

```bash
git add kernel/src/mm/page_table.rs kernel/src/mm/vmm.rs kernel/src/mm/mod.rs
git commit -m "refactor(mm): make page table walks typed"
```

---

## Task 5: Replace Global VMA Table with Per-AddressSpace State

**Files:**
- Create: `kernel/src/mm/address_space.rs`
- Modify: `kernel/src/process/mod.rs`
- Modify: `kernel/src/syscall/mod.rs`
- Modify: `kernel/src/arch/x86_64/idt.rs`

**Step 1: Define `AddressSpace`**

```rust
pub struct AddressSpace {
    root: PageTableRoot,
    vmas: VmaTable,
    next_mmap_addr: usize,
    max_elf_vaddr: usize,
}

impl AddressSpace {
    pub fn root(&self) -> PageTableRoot;
    pub fn vma_query(&self, addr: UserVirtAddr) -> Option<VmaPerms>;
    pub fn vma_is_elf(&self, addr: UserVirtAddr) -> bool;
}
```

`PageTableRoot` must be a newtype around PPN/CR3 physical root, not a plain `usize`.

**Step 2: Move VMA functions**

Move these from `kernel/src/syscall/mod.rs` into `address_space.rs`:

- `vma_add`
- `vma_update_prot`
- `vma_remove_range`
- `vma_query`
- `vma_is_elf`
- `ensure_mmap_above`
- `register_elf_vma`

**Step 3: Store address space in `Process`**

Replace:

```rust
pub page_table_root: usize,
```

with:

```rust
pub address_space: AddressSpaceHandle,
```

For clone threads, share the same handle. For fork/exec, create a new handle.

**Step 4: Update page-fault handler**

Change PF handler to fetch the current address space and query VMAs from it:

```rust
let aspace = crate::process::current_address_space();
let vma = aspace.vma_query(fault_addr);
```

No global VMA lookup should remain.

**Step 5: Run x86 build and tests**

Run:

```bash
make build-x86
make test-x86
```

Expected: build passes; test count updated if new tests are added.

**Step 6: Commit**

```bash
git add kernel/src/mm/address_space.rs kernel/src/process/mod.rs kernel/src/syscall/mod.rs kernel/src/arch/x86_64/idt.rs
git commit -m "refactor(mm): make VMAs address-space local"
```

---

## Task 6: Add RAII CR3 Guards and Remove Closure-Based CR3 Switching

**Files:**
- Create: `kernel/src/arch/x86_64/cr3.rs`
- Modify: `kernel/src/arch/x86_64/mod.rs`
- Modify: `kernel/src/arch/x86_64/trap.rs`
- Modify: `kernel/src/syscall/mod.rs`
- Modify: `kernel/src/syscall/user_ptr.rs`

**Step 1: Define CR3 newtypes**

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cr3Phys(u64);

pub struct KernelCr3(Cr3Phys);
pub struct UserCr3(Cr3Phys);
```

Only constructors in `cr3.rs` may be unsafe.

**Step 2: Define RAII guard**

```rust
#[must_use]
pub struct Cr3Guard {
    previous: Cr3Phys,
}

impl Drop for Cr3Guard {
    fn drop(&mut self) {
        unsafe { write_cr3(self.previous) };
    }
}

pub fn enter_kernel_cr3() -> Cr3Guard {
    let previous = read_cr3();
    let kernel = kernel_cr3();
    if previous != kernel {
        unsafe { write_cr3(kernel) };
    }
    Cr3Guard { previous }
}
```

**Step 3: Make guard non-sendable and non-copyable**

Add `PhantomData<*mut ()>` so the guard cannot cross threads/harts accidentally.

**Step 4: Replace `with_kernel_cr3(|| ...)`**

Before:

```rust
with_kernel_cr3(|| {
    // work
});
```

After:

```rust
let _cr3 = crate::arch::cr3::enter_kernel_cr3();
// work
```

**Step 5: Remove or deprecate `with_user_cr3`**

User-memory access should go through typed user access APIs. Do not allow arbitrary closures under user CR3.

**Step 6: Run search gate**

Run:

```bash
rg "with_user_cr3|with_kernel_cr3" kernel/src
```

Expected: no `with_user_cr3`; `with_kernel_cr3` either gone or only in a deprecated wrapper that creates `Cr3Guard`.

**Step 7: Commit**

```bash
git add kernel/src/arch/x86_64/cr3.rs kernel/src/arch/x86_64/mod.rs kernel/src/arch/x86_64/trap.rs kernel/src/syscall/mod.rs kernel/src/syscall/user_ptr.rs
git commit -m "refactor(x86_64): use RAII CR3 guards"
```

---

## Task 6A: Type x86_64 User-Return Architecture State

**Files:**
- Create: `kernel/src/arch/x86_64/user_return.rs`
- Modify: `kernel/src/arch/x86_64/mod.rs`
- Modify: `kernel/src/arch/x86_64/idt.rs`
- Modify: `kernel/src/arch/x86_64/trap.rs`
- Modify: `kernel/src/sched/mod.rs`
- Modify: `kernel/src/arch/x86_64/test.rs`

**Step 1: Define typed return-state values**

Create `kernel/src/arch/x86_64/user_return.rs`:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FsBase(u64);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UserCr3(u64);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KernelRsp0(u64);

#[derive(Clone, Copy, Debug)]
pub struct UserReturnState {
    pub user_cr3: Option<UserCr3>,
    pub kernel_rsp0: Option<KernelRsp0>,
    pub fs_base: FsBase,
}

impl FsBase {
    pub const ZERO: Self = Self(0);

    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl UserCr3 {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl KernelRsp0 {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}
```

Do not use `Option<FsBase>` for "no TLS". `FsBase::ZERO` is a real state and must be written to hardware.

**Step 2: Centralize MSR restore**

Add:

```rust
pub fn restore_fs_base(fs_base: FsBase) {
    unsafe { crate::arch::idt::wrmsr(0xC0000100, fs_base.raw()) };
}

pub fn restore_for_user_return(state: UserReturnState) {
    if let Some(cr3) = state.user_cr3 {
        unsafe { write_cr3(cr3.raw()) };
    }
    if let Some(rsp0) = state.kernel_rsp0 {
        unsafe { crate::arch::gdt::set_kernel_rsp0_for_cpu(0, rsp0.raw()) };
    }
    restore_fs_base(state.fs_base);
}
```

Wire the raw CR3 write through the CR3 module from Task 6 once it exists. Until then, keep the unsafe block local to `user_return.rs`.

**Step 3: Replace fast SYSCALL ad hoc restore**

In `kernel/src/arch/x86_64/idt.rs`, replace the direct helper with:

```rust
let state = crate::sched::current_user_return_state();
crate::arch::user_return::restore_for_user_return(state);
```

This must run after every Linux compat syscall, including simple calls such as `sched_getaffinity(204)`. The xbot regression was:

```text
0x496854 syscall        ; sched_getaffinity
0x44fa78 mov %fs:-8,%r14
PF UNHANDLED addr=0x0
```

**Step 4: Replace scheduler ad hoc arch restore**

In `kernel/src/sched/mod.rs`, store and restore a typed architecture state:

```rust
#[cfg(target_arch = "x86_64")]
pub fn current_user_return_state() -> crate::arch::user_return::UserReturnState {
    let slot = current_sched_slot();
    user_return_state_for_slot(slot)
}
```

`restore_task_arch_state()` must call the same typed restore path and must write `FsBase::ZERO` when that is the slot's stored value.

**Step 5: Update `ARCH_SET_FS`**

In `kernel/src/syscall/mod.rs`, `linux_arch_prctl(ARCH_SET_FS, addr)` should:

```rust
let fs = crate::arch::user_return::FsBase::new(addr as u64);
crate::sched::set_task_fs_base(slot, fs);
crate::arch::user_return::restore_fs_base(fs);
```

After this task, `set_task_fs_base()` should take `FsBase`, not raw `u64`. Keep a temporary raw wrapper only if migration requires it, and mark it `#[deprecated]`.

**Step 6: Add return-state tests**

Run and keep these tests:

```bash
make test-x86
```

Expected:

- `x86_64 syscall_fs_restore` passes.
- `x86_64 restore_zero_fs_base` passes.
- Existing `x86_64 fs_base_msr` and `x86_64 task_fs_base` tests still pass after being converted to `FsBase`.

**Step 7: Add xbot smoke gate**

After rebuilding the ISO, run an automated xbot startup smoke test:

```bash
make build-x86
LOG=/tmp/karte-os-x86_64-xbot-smoke.log
timeout 90s bash -lc '(sleep 5; printf "xbot-cli-static\n"; sleep 75; printf "\001x") | qemu-system-x86_64 -machine pc -cpu qemu64 -m 1024M -cdrom target/karte-os-x86_64.iso -serial stdio -display none -no-reboot -drive file=disk.img,format=raw,if=none,id=hd0 -device ich9-ahci,id=ahci -device ide-hd,drive=hd0,bus=ahci.0' > "$LOG" 2>&1 || true
rg "PF\\] UNHANDLED addr=0x0|rip=0x44fa78|mov %fs" "$LOG"
```

Expected: no matches for the fault signatures. If the command times out because the TUI keeps running, inspect the log and treat "no PF signature" as the pass condition for this smoke gate.

**Step 8: Commit**

```bash
git add kernel/src/arch/x86_64/user_return.rs kernel/src/arch/x86_64/mod.rs kernel/src/arch/x86_64/idt.rs kernel/src/arch/x86_64/trap.rs kernel/src/sched/mod.rs kernel/src/syscall/mod.rs kernel/src/arch/x86_64/test.rs
git commit -m "refactor(x86_64): type user return architecture state"
```

---

## Task 7: Replace Raw User Pointers with Capability-Based User Access

**Files:**
- Modify: `kernel/src/syscall/user_ptr.rs`
- Modify: `kernel/src/syscall/mod.rs`
- Modify: `kernel/src/driver/fs.rs`
- Modify: `kernel/src/driver/pipe.rs`
- Modify: `kernel/src/driver/tty.rs`

**Step 1: Define user access capability**

```rust
pub struct UserAccess<'a> {
    address_space: &'a AddressSpace,
    _kernel_cr3: crate::arch::cr3::Cr3Guard,
}
```

This capability means: kernel buffers are accessed under kernel CR3, and every byte copied from/to user uses a controlled CR3 transition or page-table translation.

**Step 2: Define typed user pointers**

```rust
#[repr(transparent)]
pub struct UserPtr<T> {
    addr: UserVirtAddr,
    _ty: core::marker::PhantomData<T>,
}

pub struct UserSlice<T> {
    ptr: UserPtr<T>,
    len: usize,
}
```

No syscall should directly accept a user pointer as `usize` beyond the ABI dispatch boundary.

**Step 3: Replace byte primitives**

Replace public:

```rust
pub fn user_read_u8(addr: usize) -> u8;
pub fn user_write_u8(addr: usize, value: u8);
```

with:

```rust
impl<'a> UserAccess<'a> {
    pub fn read_u8(&self, addr: UserVirtAddr) -> Result<u8, UserFault>;
    pub fn write_u8(&self, addr: UserVirtAddr, value: u8) -> Result<(), UserFault>;
}
```

Keep temporary wrappers only inside `syscall/mod.rs` for migration.

**Step 4: Update fd and pipe paths**

Change `fake_read`, `fake_write`, `pipe::read`, `pipe::write`, and TTY copy paths to take `UserSlice`/`UserSliceMut`.

**Step 5: Add compile-time search gate**

Run:

```bash
rg "read_volatile\\(\\(.*buf|write_volatile\\(\\(.*buf|as \\*mut u8|as \\*const u8" kernel/src/syscall kernel/src/driver
```

Expected: remaining raw pointer casts are either MMIO, internal kernel buffer operations, or documented `SAFETY:` blocks in user access internals.

**Step 6: Commit**

```bash
git add kernel/src/syscall/user_ptr.rs kernel/src/syscall/mod.rs kernel/src/driver/fs.rs kernel/src/driver/pipe.rs kernel/src/driver/tty.rs
git commit -m "refactor(syscall): require typed user memory access"
```

---

## Task 8: Split Mapping APIs by Mapping Kind

**Files:**
- Modify: `kernel/src/mm/vmm.rs`
- Modify: `kernel/src/mm/address_space.rs`
- Modify: `kernel/src/process/mod.rs`
- Modify: `kernel/src/syscall/mod.rs`

**Step 1: Replace generic `map_user` callsites**

Introduce explicit methods:

```rust
impl AddressSpace {
    pub fn map_elf_page(&mut self, va: UserVirtAddr, frame: OwnedFrame, perms: ElfPerms);
    pub fn map_anon_page(&mut self, va: UserVirtAddr, frame: OwnedFrame, perms: AnonPerms);
    pub fn map_stack_page(&mut self, va: UserVirtAddr, frame: OwnedFrame);
    pub fn map_kernel_trap_page(&mut self, va: KernelVirtAddr, pa: PhysAddr);
}
```

**Step 2: Define distinct permission types**

```rust
pub struct ElfPerms(PtePerms);
pub struct AnonPerms(PtePerms);
pub struct KernelPerms(PtePerms);
```

Prevent passing kernel-only perms to user mapping functions at compile time.

**Step 3: Update ELF loader**

ELF loader must allocate `OwnedFrame`, copy file bytes under kernel CR3, then `map_elf_page()`.

**Step 4: Update `mmap` and PF lazy allocation**

Anonymous `mmap` and page faults must only use `map_anon_page()`. They must never preserve identity mappings.

**Step 5: Remove public generic map where possible**

Keep low-level `vmm::map_raw()` private to `vmm.rs` or `unsafe`.

**Step 6: Run tests**

Run:

```bash
make build-x86
make test-x86
```

Expected: all VMM, syscall, and x86 tests pass.

**Step 7: Commit**

```bash
git add kernel/src/mm/vmm.rs kernel/src/mm/address_space.rs kernel/src/process/mod.rs kernel/src/syscall/mod.rs
git commit -m "refactor(mm): split mapping APIs by ownership kind"
```

---

## Task 9: Quarantine Unsafe Code Behind Audited Modules

**Files:**
- Create: `kernel/src/unsafe_api/mod.rs`
- Create: `kernel/src/unsafe_api/raw_cr3.rs`
- Create: `kernel/src/unsafe_api/raw_pte.rs`
- Create: `kernel/src/unsafe_api/raw_user_copy.rs`
- Modify: `kernel/src/main.rs`
- Modify: `kernel/src/lib.rs` if present

**Step 1: Add crate/module-level lint policy**

At crate root:

```rust
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(clippy::undocumented_unsafe_blocks)]
```

If clippy lint is too noisy for the whole crate, apply it first to the new `unsafe_api` module.

**Step 2: Move raw CR3 operations**

Only `unsafe_api/raw_cr3.rs` may contain:

```rust
asm!("mov {}, cr3", out(reg) cr3);
asm!("mov cr3, {}", in(reg) cr3);
```

**Step 3: Move raw PTE encoding/decoding**

Only `unsafe_api/raw_pte.rs` should construct PTE raw bits directly for x86_64.

**Step 4: Move user copy assembly/pointer internals**

Only `unsafe_api/raw_user_copy.rs` may temporarily dereference user virtual addresses.

**Step 5: Add search gate**

Run:

```bash
rg "unsafe \\{|asm!|\\*mut|\\*const" kernel/src
```

Expected: most remaining matches are in `unsafe_api`, arch assembly stubs, MMIO drivers, or have `SAFETY:` comments.

**Step 6: Commit**

```bash
git add kernel/src/unsafe_api kernel/src/main.rs
git commit -m "refactor(kernel): quarantine unsafe memory primitives"
```

---

## Task 10: Add Structured Memory Diagnostics

**Files:**
- Create: `kernel/src/mm/diagnostics.rs`
- Modify: `kernel/src/mm/mod.rs`
- Modify: `kernel/src/arch/x86_64/idt.rs`
- Modify: `kernel/src/syscall/mod.rs`

**Step 1: Define typed diagnostic events**

```rust
pub struct PageFaultEvent {
    pub pid: usize,
    pub tid: usize,
    pub rip: usize,
    pub fault: UserVirtAddr,
    pub access: FaultAccess,
    pub active_cr3: Cr3Phys,
    pub expected_root: PageTableRoot,
    pub fs_base: crate::arch::user_return::FsBase,
}

pub enum FaultAccess {
    Read,
    Write,
    Execute,
}
```

**Step 2: Add PTE chain dump**

```rust
pub fn dump_pte_chain(root: PageTableRoot, addr: UserVirtAddr) -> PteChain;
```

The chain should include raw entries for PML4, PDP, PD, PT, decoded leaf kind, USER/WRITABLE/NX/PS bits, and translated frame.

**Step 3: Use diagnostics in PF handler**

For any unhandled or repaired PF, log one line with:

- `pid`
- `slot`
- `root`
- `active_cr3`
- `fault`
- `rip`
- `access`
- `vma`
- `walk_result`
- `leaf_frame`
- `fs_base`
- `user_return_state`
- first 16 bytes at leaf frame when safe

For faults at low addresses (`0x0..0xfff`) from x86_64 user mode, include the current `FS_BASE` and the previous syscall return path if available. The xbot regression looked like a VMA null fault, but the true failing boundary was an incomplete user-return restore before Go executed `%fs:-8`.

**Step 4: Add mmap/mprotect diagnostics**

Log page-table mutation events:

```text
[pt-mut] op=mprotect pid=... root=... va=... before=... after=... result=...
```

Do not rate-limit. The existing policy says diagnostic logs print unconditionally.

**Step 5: Run build**

Run: `make build-x86`

Expected: build passes.

**Step 6: Commit**

```bash
git add kernel/src/mm/diagnostics.rs kernel/src/mm/mod.rs kernel/src/arch/x86_64/idt.rs kernel/src/syscall/mod.rs
git commit -m "diagnostics(mm): add typed page-table fault logs"
```

---

## Task 11: Update Architecture Documentation

**Files:**
- Modify: `docs/agent/memory.md`
- Modify: `docs/agent/trap.md`
- Modify: `docs/agent/architecture.md`
- Modify: `AGENTS.md`

**Step 1: Update memory documentation for dual architecture**

`docs/agent/memory.md` currently describes mostly RISC-V Sv39. Split it into:

- Common PMM/frame ownership
- RISC-V Sv39
- x86_64 4-level page tables
- AddressSpace/VMA ownership
- Mapping kinds

**Step 2: Update trap documentation**

Document x86_64 CR3 rules:

- Kernel code runs under kernel CR3 after context switches.
- User CR3 is installed only at explicit user return paths.
- `Cr3Guard` is required for temporary switching.
- User memory copy uses `UserAccess`, not arbitrary `with_user_cr3`.
- Linux `SYSCALL` fast return restores the full typed `UserReturnState`, including `FS_BASE`.
- `FS_BASE == 0` is valid and must be written explicitly when restoring a task that has no TLS.

**Step 3: Update AGENTS gotchas**

Replace scattered CR3/mprotect gotchas with the new invariants:

- Never walk through `PS` huge pages as page tables.
- Never expose supervisor identity mappings as user mappings.
- Never access kernel buffers under user CR3.
- Never add USER to copied identity leaves.
- Never return to x86_64 user mode without restoring CR3 and `FS_BASE`.
- Never treat `FS_BASE == 0` as "do not update the MSR".

**Step 4: Commit**

```bash
git add docs/agent/memory.md docs/agent/trap.md docs/agent/architecture.md AGENTS.md
git commit -m "docs(mm): document typed memory invariants"
```

---

## Task 12: Final Migration Gates

**Files:**
- Modify as needed based on search results.

**Step 1: Raw user-pointer gate**

Run:

```bash
rg "user_read_u8\\(|user_write_u8\\(|with_user_cr3|with_kernel_cr3" kernel/src
```

Expected:

- No `with_user_cr3`.
- `with_kernel_cr3` removed or deprecated in favor of `Cr3Guard`.
- Raw byte user APIs only inside `user_ptr.rs` or temporary compatibility wrappers.

**Step 2: Raw address gate**

Run:

```bash
rg "page_table_root: usize|current_page_table_root\\(\\) -> usize|addr: usize|vaddr: usize|paddr: usize" kernel/src/mm kernel/src/process kernel/src/syscall
```

Expected:

- Public memory APIs use typed address/root types.
- Raw `usize` appears only at syscall ABI boundary or low-level encoding boundaries.

**Step 3: Huge-page gate**

Run:

```bash
rg "PS\\)|PTEFlags::PS|contains\\(PTEFlags::PS\\)" kernel/src/mm kernel/src/arch
```

Expected:

- Every huge-page case returns `WalkResult::MappedHuge`, splits explicitly, or no-ops explicitly.

**Step 4: Full verification**

Run:

```bash
cd user && make clean && make
cargo fmt
cargo build --release -p karte-os-kernel
make test
make test-x86
make build-x86
```

Expected:

- Kernel builds.
- RISC-V tests retain existing pass count.
- x86_64 tests retain existing pass count or improve if PMM layout test is fixed.
- x86 ISO is produced.

**Step 5: xbot TLS smoke verification**

Run:

```bash
LOG=/tmp/karte-os-x86_64-xbot-final.log
timeout 90s bash -lc '(sleep 5; printf "xbot-cli-static\n"; sleep 75; printf "\001x") | qemu-system-x86_64 -machine pc -cpu qemu64 -m 1024M -cdrom target/karte-os-x86_64.iso -serial stdio -display none -no-reboot -drive file=disk.img,format=raw,if=none,id=hd0 -device ich9-ahci,id=ahci -device ide-hd,drive=hd0,bus=ahci.0' > "$LOG" 2>&1 || true
rg "PF\\] UNHANDLED addr=0x0|rip=0x44fa78|KERN FATAL|GP FAULT" "$LOG"
```

Expected: no matches. A timeout is acceptable only if the xbot TUI is still alive and the log has no PF/GP/kernel-fatal signature.

**Step 6: User-return state search gate**

Run:

```bash
rg "wrmsr\\(0xC0000100|PENDING_FS_BASE|TASK_FS_BASE|restore_user_cr3|iretq|sysret|SYSCALL" kernel/src/arch/x86_64 kernel/src/sched kernel/src/syscall
```

Expected:

- Raw `FS_BASE` MSR writes are confined to `user_return.rs` or a temporary compatibility shim.
- Every x86_64 user-return path goes through the typed `UserReturnState` restore helper.
- No restore function skips writing `FS_BASE` when the stored value is zero.

**Step 7: Commit final cleanup**

```bash
git add .
git commit -m "refactor(mm): finish typed memory migration"
```

---

## Review Checklist

- [ ] No syscall or driver code writes to user buffers through raw pointers.
- [ ] No arbitrary closure can run under user CR3.
- [ ] CR3 switches are RAII guarded.
- [ ] Every x86_64 user-return path restores typed `UserReturnState`.
- [ ] `FS_BASE == 0` is restored explicitly and is not treated as "no TLS update".
- [ ] VMA state is address-space local.
- [ ] Huge page walks are explicit in type signatures or enum results.
- [ ] ELF mappings cannot be mistaken for anonymous mappings.
- [ ] Anonymous mappings cannot preserve identity frames.
- [ ] `mprotect` never descends into a huge page.
- [ ] PF handler logs typed walk state before repair or termination.
- [ ] Low-address user PF logs include `FS_BASE` and enough syscall-return context to distinguish TLS loss from a real null pointer.
- [ ] xbot startup smoke has no `PF UNHANDLED addr=0x0 rip=0x44fa78` regression.
- [ ] Docs match current x86_64 behavior.

## Execution Notes

Do this in small commits. The safest order is tests first, then type wrappers, then ownership, then API migration. The exception is x86_64 user-return architecture state: CR3 and `FS_BASE` must be treated as one return-to-user boundary, so do not refactor memory return paths while leaving TLS restore as an ad hoc side effect. Avoid changing scheduler policy while refactoring memory APIs unless a test proves scheduler arch-state save/restore is part of the bug.

Plan complete and saved to `docs/plans/2026-06-11-memory-cr3-type-safety-refactor.md`.

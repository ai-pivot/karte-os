//! Address-space scoped VMA (Virtual Memory Area) management.
//!
//! VMA/mm state is scoped by `page_table_root` (PPN). This provides:
//! - Isolation: different processes have independent VMA state
//! - CLONE_VM sharing: threads sharing a page table root share VMA state
//! - Trap-safe lookup: PF handler queries by `current_page_table_root()`
//!   without taking PROCESS_TABLE lock
//!
//! Lifecycle:
//! - `init_root(root)`: called when ELF loader creates a new page table
//! - `release_root(root)`: called when page table is freed (reclaim_process)
//! - `clone_root_state(src, dst)`: called during fork
//! - CLONE_VM threads: no special handling — they share the same root

/// Maximum number of VMA regions per address space.
const MAX_VMAS: usize = 1024;

/// Maximum number of address spaces (one per process).
const MAX_ADDRESS_SPACES: usize = crate::sched::MAX_TASKS;

/// A single VMA region descriptor.
#[derive(Clone, Copy)]
struct VmaRegion {
    start: usize,
    end: usize,  // exclusive (first byte past the region)
    prot: usize, // PROT_* bit flags (0 = PROT_NONE)
    active: bool,
}

impl VmaRegion {
    const fn empty() -> Self {
        VmaRegion {
            start: 0,
            end: 0,
            prot: 0,
            active: false,
        }
    }
}

/// Per-address-space VMA state.
pub struct VmState {
    vmas: [VmaRegion; MAX_VMAS],
    next_mmap_addr: usize,
    max_elf_vaddr: usize,
}

impl VmState {
    fn clear(&mut self) {
        for vma in self.vmas.iter_mut() {
            *vma = VmaRegion::empty();
        }
        self.next_mmap_addr = 0;
        self.max_elf_vaddr = 0;
    }

    fn copy_from(&mut self, other: &VmState) {
        for i in 0..MAX_VMAS {
            self.vmas[i] = other.vmas[i];
        }
        self.next_mmap_addr = other.next_mmap_addr;
        self.max_elf_vaddr = other.max_elf_vaddr;
    }
}

/// Upper bound of deliberate stack scratch space used by this module.
///
/// The VMA tables themselves live in the static registry. Keep mutations
/// in-place: x86_64 user tasks only have a 32KB kernel stack.
pub const fn max_stack_scratch_bytes() -> usize {
    0
}

/// A slot in the VM state registry, keyed by page_table_root.
struct VmStateSlot {
    root: usize,
    active: bool,
    state: VmState,
}

const fn vm_state_slot_default() -> VmStateSlot {
    VmStateSlot {
        root: 0,
        active: false,
        state: VmState {
            vmas: [VmaRegion::empty(); MAX_VMAS],
            next_mmap_addr: 0,
            max_elf_vaddr: 0,
        },
    }
}

static VM_STATES: spin::Mutex<[VmStateSlot; MAX_ADDRESS_SPACES]> =
    spin::Mutex::new([const { vm_state_slot_default() }; MAX_ADDRESS_SPACES]);

// ─── Registry management ────────────────────────────────────────────

/// Initialize a new address space's VMA state.
/// Called when a new page table is created (ELF loader, fork).
/// Returns Err if no free slot is available.
pub fn init_root(root: usize) -> Result<(), ()> {
    if root == 0 {
        return Err(());
    }
    let mut registry = VM_STATES.lock();
    // Check if this root is already registered (shouldn't happen, but be safe)
    for slot in registry.iter_mut() {
        if slot.active && slot.root == root {
            // Already initialized — clear and reinit
            slot.state.clear();
            return Ok(());
        }
    }
    // Find a free slot
    for slot in registry.iter_mut() {
        if !slot.active {
            slot.root = root;
            slot.active = true;
            slot.state.clear();
            return Ok(());
        }
    }
    crate::console_println!("[VMA] init_root({:#x}): no free slot!", root);
    Err(())
}

/// Release an address space's VMA state.
/// Called when the page table is freed (reclaim_process for non-shared roots).
pub fn release_root(root: usize) {
    if root == 0 {
        return;
    }
    let mut registry = VM_STATES.lock();
    for slot in registry.iter_mut() {
        if slot.active && slot.root == root {
            slot.active = false;
            slot.root = 0;
            slot.state.clear();
            return;
        }
    }
}

/// Clone VMA state from src_root to dst_root.
/// Called during fork: child gets an independent copy of parent's VMA state.
pub fn clone_root_state(src_root: usize, dst_root: usize) -> Result<(), ()> {
    if src_root == 0 || dst_root == 0 {
        return Err(());
    }
    let mut registry = VM_STATES.lock();
    let mut src_slot_idx: Option<usize> = None;
    let mut dst_slot_idx: Option<usize> = None;

    for (i, slot) in registry.iter().enumerate() {
        if slot.active && slot.root == src_root {
            src_slot_idx = Some(i);
        }
        if !slot.active && dst_slot_idx.is_none() {
            dst_slot_idx = Some(i);
        }
    }

    let src_idx = match src_slot_idx {
        Some(i) => i,
        None => return Err(()),
    };
    let dst_idx = match dst_slot_idx {
        Some(i) => i,
        None => return Err(()),
    };

    if src_idx < dst_idx {
        let (left, right) = registry.split_at_mut(dst_idx);
        right[0].state.copy_from(&left[src_idx].state);
    } else if src_idx > dst_idx {
        let (left, right) = registry.split_at_mut(src_idx);
        left[dst_idx].state.copy_from(&right[0].state);
    }
    registry[dst_idx].root = dst_root;
    registry[dst_idx].active = true;
    Ok(())
}

/// Find the VmState for a given root, executing a closure with mutable access.
/// Returns Err(()) if root not found.
fn with_state<F, R>(root: usize, f: F) -> Result<R, ()>
where
    F: FnOnce(&mut VmState) -> R,
{
    let mut registry = VM_STATES.lock();
    for slot in registry.iter_mut() {
        if slot.active && slot.root == root {
            return Ok(f(&mut slot.state));
        }
    }
    Err(())
}

/// Find the VmState for a given root, executing a closure with read access.
/// Returns Err(()) if root not found.
fn with_state_ref<F, R>(root: usize, f: F) -> Result<R, ()>
where
    F: FnOnce(&VmState) -> R,
{
    let registry = VM_STATES.lock();
    for slot in registry.iter() {
        if slot.active && slot.root == root {
            return Ok(f(&slot.state));
        }
    }
    Err(())
}

// ─── VMA query/modify API ────────────────────────────────────────────

/// Check if `addr` falls within a VMA that permits access (prot != PROT_NONE).
/// Returns Some(prot) if valid, None if no VMA covers this address or VMA is PROT_NONE.
pub fn vma_check(root: usize, addr: usize) -> Option<usize> {
    with_state_ref(root, |state| {
        let mut best_prot: Option<usize> = None;
        let mut best_size: usize = usize::MAX;
        for vma in state.vmas.iter() {
            if vma.active && addr >= vma.start && addr < vma.end {
                let size = vma.end - vma.start;
                let cur_accessible = vma.prot != 0;
                let best_accessible = best_prot.map_or(false, |p| p != 0);
                if (cur_accessible && !best_accessible)
                    || (cur_accessible == best_accessible && size < best_size)
                {
                    best_size = size;
                    best_prot = if vma.prot == 0 { None } else { Some(vma.prot) };
                }
            }
        }
        best_prot
    })
    .unwrap_or(None)
}

/// Query VMA protection for `addr` — distinguishes PROT_NONE from no-VMA.
/// Returns `Some(prot)` if a VMA covers this address (prot may be 0 for PROT_NONE).
/// Returns `None` if no VMA covers this address at all.
pub fn vma_query(root: usize, addr: usize) -> Option<usize> {
    with_state_ref(root, |state| {
        for vma in state.vmas.iter() {
            if vma.active && addr >= vma.start && addr < vma.end {
                return Some(vma.prot);
            }
        }
        None
    })
    .unwrap_or(None)
}

/// Dump VMA entries near a given address for debugging (root-aware).
pub fn vma_dump_region(root: usize, addr: usize) {
    let _ = with_state_ref(root, |state| {
        let range = 64 * 1024 * 1024; // ±64MB
        let mut count = 0;
        let mut total_active = 0;
        for vma in state.vmas.iter() {
            if vma.active {
                total_active += 1;
                if vma.start < addr + range && vma.end > addr.saturating_sub(range) {
                    let contains = addr >= vma.start && addr < vma.end;
                    crate::console_println!(
                        "[VMA] root={:#x} {:#x}..{:#x} prot={:#x} {}",
                        root,
                        vma.start,
                        vma.end,
                        vma.prot,
                        if contains { "<<< CONTAINS fault" } else { "" }
                    );
                    count += 1;
                    if count >= 20 {
                        break;
                    }
                }
            }
        }
        crate::console_println!(
            "[VMA] root={:#x} total active={}/{} shown={}",
            root,
            total_active,
            MAX_VMAS,
            count
        );
    });
}

/// Check if [start, end) overlaps with any active VMA entry in the given root.
pub fn vma_overlaps(root: usize, start: usize, end: usize) -> bool {
    with_state_ref(root, |state| {
        for vma in state.vmas.iter() {
            if vma.active && vma.start < end && vma.end > start {
                return true;
            }
        }
        false
    })
    .unwrap_or(false)
}

fn insert_vma(
    vmas: &mut [VmaRegion; MAX_VMAS],
    start: usize,
    end: usize,
    prot: usize,
) -> Result<(), ()> {
    if start >= end {
        return Ok(());
    }
    for vma in vmas.iter_mut() {
        if !vma.active {
            *vma = VmaRegion {
                start,
                end,
                prot,
                active: true,
            };
            return Ok(());
        }
    }
    Err(())
}

/// Split or remove VMA entries that overlap with [start, end).
fn split_overlapping_vmas(
    vmas: &mut [VmaRegion; MAX_VMAS],
    start: usize,
    end: usize,
) -> Result<(), ()> {
    for i in 0..MAX_VMAS {
        let vma = &vmas[i];
        if !vma.active || vma.start >= end || vma.end <= start {
            continue;
        }
        let vma_end = vma.end;
        let vma_prot = vma.prot;

        if vma.start < start && vma.end > end {
            // Fully contains: split into [vma_start, start) + [end, vma_end)
            vmas[i].end = start;
            insert_vma(vmas, end, vma_end, vma_prot)?;
        } else if vma.start < start {
            // Overlaps tail: truncate
            vmas[i].end = start;
        } else if vma.end > end {
            // Overlaps head: move start
            vmas[i].start = end;
        } else {
            // Fully covered: deactivate
            vmas[i].active = false;
        }
    }
    Ok(())
}

/// Add or update a VMA entry for [start, end) with the given prot.
/// For MAP_FIXED, removes any overlapping entries first.
/// Returns Ok(()) on success, Err(()) if no free VMA slot is available or root not found.
pub fn vma_add(
    root: usize,
    start: usize,
    end: usize,
    prot: usize,
    map_fixed: bool,
) -> Result<(), ()> {
    with_state(root, |state| {
        if map_fixed {
            split_overlapping_vmas(&mut state.vmas, start, end)?;
        }
        insert_vma(&mut state.vmas, start, end, prot)
    })?
}

/// Remove all VMA entries overlapping [start, end).
/// Re-inserts tail fragments (portions of VMAs outside the removed range).
pub fn vma_remove_range(root: usize, start: usize, end: usize) {
    let _ = with_state(root, |state| {
        let _ = split_overlapping_vmas(&mut state.vmas, start, end);
    });
}

/// Update prot for all VMA entries overlapping [start, end).
pub fn vma_update_prot(root: usize, start: usize, end: usize, new_prot: usize) {
    let _ = with_state(root, |state| {
        for vma in state.vmas.iter_mut() {
            if vma.active && vma.start < end && vma.end > start {
                vma.prot = new_prot;
            }
        }
    });
}

/// Ensure the mmap bump allocator starts at or above `min_addr`.
/// Called by the ELF loader after loading segments.
pub fn ensure_mmap_above(root: usize, min_addr: usize) {
    let aligned = (min_addr + 4095) & !4095;
    let _ = with_state(root, |state| {
        if state.next_mmap_addr < aligned {
            state.next_mmap_addr = aligned;
        }
        if state.max_elf_vaddr < aligned {
            state.max_elf_vaddr = aligned;
        }
    });
}

/// Register an ELF PT_LOAD segment as a VMA entry.
pub fn register_elf_vma(root: usize, start: usize, end: usize, prot: usize) {
    let _ = vma_add(root, start, end, prot, false);
}

/// Return true when `addr` belongs to the ELF PT_LOAD address range.
pub fn vma_is_elf(root: usize, addr: usize) -> bool {
    with_state_ref(root, |state| {
        state.max_elf_vaddr != 0
            && addr >= 0x400000
            && addr < state.max_elf_vaddr
            && state
                .vmas
                .iter()
                .any(|v| v.active && addr >= v.start && addr < v.end)
    })
    .unwrap_or(false)
}

/// Reserve a contiguous mmap address range of the given length.
/// Returns the start address, or Err if no suitable range is found.
/// Uses per-root bump allocator starting from next_mmap_addr or USER_MMAP_BASE.
pub fn reserve_mmap_addr(root: usize, len: usize) -> Result<usize, ()> {
    let page_size = crate::mm::pmm::page_size();
    let aligned_len = (len + page_size - 1) & !(page_size - 1);
    with_state(root, |state| {
        let base = crate::process::USER_MMAP_BASE;
        let limit = crate::process::USER_MMAP_LIMIT;
        if state.next_mmap_addr == 0 || state.next_mmap_addr < base {
            state.next_mmap_addr = limit;
        }
        loop {
            let candidate = (state.next_mmap_addr.saturating_sub(aligned_len)) & !(page_size - 1);
            if candidate < base {
                return Err(());
            }
            let overlaps = state
                .vmas
                .iter()
                .any(|v| v.active && v.start < state.next_mmap_addr && v.end > candidate);
            if overlaps {
                let cs = state
                    .vmas
                    .iter()
                    .filter(|v| v.active && v.start < state.next_mmap_addr && v.end > candidate)
                    .map(|v| v.start)
                    .min()
                    .unwrap_or(candidate);
                state.next_mmap_addr = cs & !(page_size - 1);
                continue;
            }
            state.next_mmap_addr = candidate;
            return Ok(candidate);
        }
    })?
}
/// Clear all VMA state for a given root (used by exec which replaces address space).
pub fn vma_clear_root(root: usize) {
    let _ = with_state(root, |state| {
        state.clear();
    });
}

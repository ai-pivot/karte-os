//! Process management: user-space processes with independent address spaces.

pub mod elf;

use alloc::collections::BTreeMap;
use alloc::string::String;

use core::sync::atomic::{AtomicUsize, Ordering};

use spin::Mutex;

use crate::mm::{pmm, vmm};

/// User address space layout constants.
///
/// x86_64 address space layout:
///   0x0000_0000 .. 0x0040_0000 = kernel (identity mapped, first 4MB)
///   0x0040_0000 .. 0x1000_0000 = user code + data (up to 252MB for Go binary)
///   0x1000_0000 .. 0x2000_0000 = user heap (brk, 256MB)
///   0x2000_0000 .. 0x8000_0000 = user mmap (1.5GB)
///   0x7FC0_0000 .. 0x8000_0000 = user stack (4MB, top at 2GB)
///   0xF000_0000 .. 0x1_0000_0000 = PCI MMIO (kernel only)
///   0xFEC0_0000, 0xFEE0_0000    = IOAPIC, LAPIC (kernel only)
pub const USER_CODE_BASE: usize = 0x0000_0000;
pub const USER_CODE_LIMIT: usize = 0x1000_0000; // 256MB for code+data
pub const USER_HEAP_BASE: usize = 0x1000_0000;
pub const USER_HEAP_LIMIT: usize = 0x2000_0000; // 256MB heap (brk)
pub const USER_MMAP_BASE: usize = 0x2000_0000; // Above heap (256MB), separate from brk region
pub const USER_MMAP_LIMIT: usize = 0x0000_7FFF_FFFF_F000; // x86_64 max canonical user address
pub const USER_STACK_TOP: usize = 0x8000_0000; // 2GB — top of user stack
pub const USER_STACK_BASE: usize = 0x7F00_0000; // 16MB stack region (address space)
pub const USER_STACK_PAGES: usize = 512; // 2 MB pre-mapped stack (Go g0 needs ~1MB+)
pub const KERNEL_STACK_PAGES: usize = 8; // 32 KB kernel stack
#[cfg(target_arch = "x86_64")]
const CET_TRANSITION_STACK_PAGE: usize = 0xffff_ffff_ffff_f000;
#[cfg(target_arch = "x86_64")]
pub const KERNEL_STACK_VIRT_BASE: usize = 0xffff_8000_0000_0000;

#[cfg(target_arch = "x86_64")]
pub(crate) fn kernel_stack_top_from_phys_base(phys_base: usize) -> usize {
    KERNEL_STACK_VIRT_BASE + phys_base + KERNEL_STACK_PAGES * pmm::page_size()
}

#[cfg(target_arch = "x86_64")]
fn kernel_stack_phys_base_from_top(kernel_stack_top: usize) -> usize {
    kernel_stack_top - KERNEL_STACK_VIRT_BASE - KERNEL_STACK_PAGES * pmm::page_size()
}

#[cfg(target_arch = "x86_64")]
fn map_kernel_stack_pages_at(
    page_table: &mut vmm::PageTable,
    phys_base: usize,
    kernel_stack_top: usize,
) {
    let page_size = pmm::page_size();
    let virt_base = kernel_stack_top - KERNEL_STACK_PAGES * page_size;
    for offset in (0..KERNEL_STACK_PAGES * page_size).step_by(page_size) {
        vmm::map(
            page_table,
            virt_base + offset,
            phys_base + offset,
            vmm::PTEFlags::KRW,
        );
    }
}

pub(crate) fn map_kernel_stack_pages(user_pt: &mut vmm::PageTable, kernel_stack_top: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        let phys_base = kernel_stack_phys_base_from_top(kernel_stack_top);
        map_kernel_stack_pages_at(user_pt, phys_base, kernel_stack_top);
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        let page_size = pmm::page_size();
        let kstack_base = kernel_stack_top - KERNEL_STACK_PAGES * page_size;
        for addr in (kstack_base..kernel_stack_top).step_by(page_size) {
            vmm::map(user_pt, addr, addr, vmm::PTEFlags::KRW);
        }
    }
}

pub fn alloc_kernel_stack() -> Option<usize> {
    let phys_base = pmm::alloc_contiguous_frames(KERNEL_STACK_PAGES)?;

    #[cfg(target_arch = "x86_64")]
    {
        let kernel_stack_top = kernel_stack_top_from_phys_base(phys_base);
        let kernel_pt = vmm::get_kernel_page_table();
        map_kernel_stack_pages_at(kernel_pt, phys_base, kernel_stack_top);
        Some(kernel_stack_top)
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        Some(phys_base + KERNEL_STACK_PAGES * pmm::page_size())
    }
}

fn dealloc_kernel_stack(kernel_stack_top: usize) {
    #[cfg(target_arch = "x86_64")]
    let phys_base = kernel_stack_phys_base_from_top(kernel_stack_top);

    #[cfg(not(target_arch = "x86_64"))]
    let phys_base = kernel_stack_top - KERNEL_STACK_PAGES * pmm::page_size();

    for offset in (0..KERNEL_STACK_PAGES * pmm::page_size()).step_by(pmm::page_size()) {
        pmm::dealloc_frame(phys_base + offset);
    }
}

/// Process identifier allocator
pub(crate) static NEXT_PID: AtomicUsize = AtomicUsize::new(1);

/// Maximum number of processes in the system
const MAX_PROCESSES: usize = 64;

/// Process state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProcessState {
    Ready,
    Running,
    Blocked,
    Exited,
}

/// Process control block
#[derive(Clone)]
pub struct Process {
    pub pid: usize,
    /// Parent process ID (0 for init process)
    pub ppid: usize,
    /// Root page table PPN (physical page number of the Sv39 root page table)
    pub page_table_root: usize,
    /// Kernel stack top (used for trap handling when in U-mode)
    pub kernel_stack_top: usize,
    /// FS_BASE MSR value for TLS (set by clone ARCH_SETTLS)
    pub fs_base: u64,
    /// User stack top
    pub user_stack_top: usize,
    /// Program break (top of heap)
    pub brk: usize,
    /// Initial brk (end of loaded ELF data)
    pub initial_brk: usize,
    /// Entry point (virtual address)
    pub entry: usize,
    /// Current process state
    pub state: ProcessState,
    /// Exit code (valid when state == Exited)
    pub exit_code: usize,
    /// Index in PROCESS_TABLE of the child this process is waiting for.
    /// None if not waiting. Used by sys_exit to wake the parent.
    pub wait_child_idx: Option<usize>,
    /// Per-process file descriptor table.
    /// Arc-shared so CLONE_FILES threads see the same table.
    pub fd_table: alloc::sync::Arc<spin::Mutex<crate::driver::fs::FdTable>>,
    /// Trap context pointer (saved on kernel stack)
    pub trap_ctx_ptr: usize,
    /// Whether this process's page table is shared via CLONE_VM.
    /// If true, reclaim_process will NOT free the page table.
    pub shared_page_table: bool,
    /// TLS address for CLONE_SETTLS (x86_64: IA32_FS_BASE).
    /// Applied on first entry via clone_first_shim. 0 = no TLS.
    pub clone_tls: usize,
    /// Child TID pointer for CLONE_CHILD_CLEARTID.
    /// When this process exits, kernel writes 0 to this address.
    pub child_tid_ptr: usize,
    /// Per-process environment variables.
    /// Inherited from parent on fork/clone, replaced on execve with envp.
    pub env: BTreeMap<String, String>,
}

/// Copy kernel identity mappings into a user page table.
/// This is needed so that traps from U-mode can still access kernel code/data.
pub(crate) fn copy_kernel_mappings(user_pt: &mut vmm::PageTable, kernel_stack_top: usize) {
    #[cfg(target_arch = "riscv64")]
    {
        // Identity map kernel (0x80200000 .. 0x80200000 + 128MB)
        vmm::identity_map(
            user_pt,
            0x8020_0000,
            0x8020_0000 + 128 * 1024 * 1024,
            vmm::PTEFlags::KRWX,
        );

        // Map UART MMIO (0x10000000 - 0x10001000)
        vmm::map(user_pt, 0x1000_0000, 0x1000_0000, vmm::PTEFlags::KRW);

        // Map VirtIO MMIO devices (0x10001000 - 0x10009000)
        for addr in (0x1000_1000..0x1000_9000).step_by(4096) {
            vmm::map(user_pt, addr, addr, vmm::PTEFlags::KRW);
        }

        // Map PLIC (0x0C000000 - 0x0C400000)
        for addr in (0x0C00_0000..0x0C40_0000).step_by(4096) {
            vmm::map(user_pt, addr, addr, vmm::PTEFlags::KRW);
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        // Copy kernel's identity map (2MB huge pages at PD level with PS=1)
        // to the user page table. This establishes identity-mapped access to
        // all physical memory using 2MB huge pages, avoiding the 4KB-PTE
        // bootstrapping problem (where PT tables created at high physical
        // addresses aren't yet identity-mapped themselves).
        //
        // Each kernel PD table is copied independently: we create a new PD
        // table, clone kernel PD entries, and add USER bit. This keeps kernel
        // and user page tables fully isolated (no shared tables).
        let kernel_pt = vmm::get_kernel_page_table();
        let ke = kernel_pt.entries[0];
        if ke.is_valid() {
            let kernel_pdp = unsafe { &mut *((ke.ppn() << 12) as *mut vmm::PageTable) };
            let user_pml4 = &mut user_pt.entries[0];
            let user_pdp_ppn = if user_pml4.is_valid() {
                user_pml4.ppn()
            } else {
                let new_pdp = vmm::PageTable::zeroed();
                let ppn = (new_pdp as *const vmm::PageTable as usize) >> 12;
                let flags = vmm::PTEFlags::PRESENT.bits()
                    | vmm::PTEFlags::WRITABLE.bits()
                    | vmm::PTEFlags::USER.bits();
                *user_pml4 = vmm::PTE(((ppn as u64) << 12) | flags);
                ppn
            };
            let user_pdp = unsafe { &mut *((user_pdp_ppn << 12) as *mut vmm::PageTable) };
            let user_flag = vmm::PTEFlags::USER.bits();
            // CRITICAL: Only copy PDP entries that point to PD tables (PS=0).
            // Skip 1GB huge page entries (PS=1) — these are MMIO ranges
            // (LAPIC at 0xFEE00000, etc.) which must NOT be user-accessible.
            const PS_BIT: u64 = 0x80;
            for i in 0..512 {
                let kpe = kernel_pdp.entries[i];
                // Skip non-present and 1GB huge pages (PS=1)
                if !kpe.is_valid() || (kpe.0 & PS_BIT) != 0 {
                    continue;
                }
                if user_pdp.entries[i].is_valid() {
                    continue;
                }
                let kernel_pd = unsafe { &mut *((kpe.ppn() << 12) as *mut vmm::PageTable) };
                let new_pd = vmm::PageTable::zeroed();
                let new_pd_ppn = (new_pd as *const vmm::PageTable as usize) >> 12;
                for j in 0..512 {
                    let pd_entry = kernel_pd.entries[j];
                    if pd_entry.is_valid() {
                        // Leaf kernel/identity mappings must remain supervisor-only.
                        // Non-leaf entries need USER so Ring 3 page walks can reach
                        // user leaves that we install later in the same subtree.
                        let copied = if pd_entry.flags().contains(vmm::PTEFlags::PS) {
                            pd_entry.0 & !user_flag
                        } else {
                            pd_entry.0 | user_flag
                        };
                        new_pd.entries[j] = vmm::PTE(copied);
                    }
                }
                let pd_flags = vmm::PTEFlags::PRESENT.bits()
                    | vmm::PTEFlags::WRITABLE.bits()
                    | vmm::PTEFlags::USER.bits();
                user_pdp.entries[i] = vmm::PTE(((new_pd_ppn as u64) << 12) | pd_flags);
            }
        }

        // Map VGA text buffer at 0xB8000 (needed for console_putchar in trap path)
        vmm::map(user_pt, 0xB8000, 0xB8000, vmm::PTEFlags::KRW);
        vmm::map(user_pt, 0xB9000, 0xB9000, vmm::PTEFlags::KRW);
        // NOTE: LAPIC (0xFEE00000), IOAPIC (0xFEC00000), PCI MMIO are NOT
        // mapped into user page tables. Kernel accesses them via with_kernel_cr3().

        // Map kernel stack pages into user page table.
        map_kernel_stack_pages(user_pt, kernel_stack_top);

        // Some x86_64 emulators/CPUs can perform a CET/shadow-stack transition
        // access while the user CR3 is already active but before Ring 3 code
        // runs. Keep the canonical top shadow-stack page mapped supervisor-only
        // in every user page table. User mode cannot access it because the leaf
        // PTE intentionally lacks the USER bit.
        if vmm::translate_user(user_pt, CET_TRANSITION_STACK_PAGE).is_none() {
            let transition_frame =
                pmm::alloc_frame().expect("Out of memory for CET transition page");
            vmm::map(
                user_pt,
                CET_TRANSITION_STACK_PAGE,
                transition_frame,
                vmm::PTEFlags::KRW,
            );
        }
    }
}

fn elf_segment_pte_flags(flags: usize) -> vmm::PTEFlags {
    let executable = flags & 1 != 0; // PF_X
    let writable = flags & 2 != 0; // PF_W
    let readable = flags & 4 != 0; // PF_R

    #[cfg(target_arch = "x86_64")]
    {
        let mut pte_flags = vmm::PTEFlags::PRESENT | vmm::PTEFlags::USER;
        if writable {
            pte_flags |= vmm::PTEFlags::WRITABLE;
        }
        if !executable {
            pte_flags |= vmm::PTEFlags::NX;
        }
        pte_flags
    }

    #[cfg(target_arch = "riscv64")]
    {
        let mut pte_flags = vmm::PTEFlags::V | vmm::PTEFlags::U;
        if readable {
            pte_flags |= vmm::PTEFlags::R;
        }
        if writable {
            pte_flags |= vmm::PTEFlags::W;
        }
        if executable {
            pte_flags |= vmm::PTEFlags::X;
        }
        pte_flags
    }
}

fn merge_page_flags(
    user_pt: &mut vmm::PageTable,
    vaddr: usize,
    new_flags: vmm::PTEFlags,
) -> vmm::PTEFlags {
    #[cfg(target_arch = "x86_64")]
    {
        if let Some(raw) = vmm::debug_pte(user_pt, vaddr) {
            let existing = vmm::PTEFlags::from_bits_truncate(raw);
            let mut merged = existing | new_flags;
            // NX is a restrictive bit: if any segment sharing this page is
            // executable, the page must remain executable.
            if !existing.contains(vmm::PTEFlags::NX) || !new_flags.contains(vmm::PTEFlags::NX) {
                merged.remove(vmm::PTEFlags::NX);
            }
            return merged;
        }
    }

    #[cfg(target_arch = "riscv64")]
    {
        let _ = user_pt;
        let _ = vaddr;
    }

    new_flags
}

#[cfg(target_arch = "x86_64")]
fn merge_pte_flags(existing: vmm::PTEFlags, new_flags: vmm::PTEFlags) -> vmm::PTEFlags {
    let mut merged = existing | new_flags;
    // NX is restrictive: any executable segment sharing the page must keep it executable.
    if !existing.contains(vmm::PTEFlags::NX) || !new_flags.contains(vmm::PTEFlags::NX) {
        merged.remove(vmm::PTEFlags::NX);
    }
    merged
}

#[cfg(target_arch = "x86_64")]
fn streaming_elf_page_flags(
    elf_info: &elf::ElfInfo,
    page_addr: usize,
    page_size: usize,
) -> Option<vmm::PTEFlags> {
    let mut merged: Option<vmm::PTEFlags> = None;
    for seg_idx in 0..elf_info.num_segments {
        let seg = elf_info.segments[seg_idx].as_ref().unwrap();
        let page_start = seg.vaddr & !(page_size - 1);
        let page_end = (seg.vaddr + seg.mem_size + page_size - 1) & !(page_size - 1);
        if page_addr < page_start || page_addr >= page_end {
            continue;
        }

        let flags = elf_segment_pte_flags(seg.flags as usize);
        merged = Some(match merged {
            Some(existing) => merge_pte_flags(existing, flags),
            None => flags,
        });
    }
    merged
}

#[cfg(target_arch = "x86_64")]
fn reload_streaming_elf_page<F>(
    user_pt: &mut vmm::PageTable,
    elf_info: &elf::ElfInfo,
    read_fn: &F,
    page_addr: usize,
    page_size: usize,
) -> Result<(), &'static str>
where
    F: Fn(usize, &mut [u8]) -> Result<usize, ()>,
{
    let flags = streaming_elf_page_flags(elf_info, page_addr, page_size)
        .ok_or("ELF: page not in segment")?;
    let frame = pmm::alloc_frame().ok_or("Out of memory for ELF invariant repair")?;
    vmm::map_user(user_pt, page_addr, frame, flags);
    unsafe {
        core::ptr::write_bytes(frame as *mut u8, 0, page_size);
    }

    for seg_idx in 0..elf_info.num_segments {
        let seg = elf_info.segments[seg_idx].as_ref().unwrap();
        let seg_start = seg.vaddr;
        let seg_file_end = seg.vaddr + seg.file_size;
        let seg_mem_end = seg.vaddr + seg.mem_size;
        if page_addr >= seg_mem_end || page_addr + page_size <= seg_start {
            continue;
        }

        let copy_start = core::cmp::max(page_addr, seg_start);
        let copy_end = core::cmp::min(page_addr + page_size, seg_file_end);
        if copy_start >= copy_end {
            continue;
        }

        let file_offset = seg.offset + (copy_start - seg_start);
        let dst_offset = copy_start & (page_size - 1);
        let len = copy_end - copy_start;
        let mut tmp_buf = [0u8; 4096];
        let bytes = read_fn(file_offset, &mut tmp_buf[..len])
            .map_err(|_| "ELF: failed to reread invariant page")?;
        if bytes < len {
            unsafe {
                core::ptr::write_bytes(tmp_buf[bytes..].as_mut_ptr(), 0, len - bytes);
            }
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                tmp_buf[..len].as_ptr(),
                (frame + dst_offset) as *mut u8,
                len,
            );
        }
    }

    Ok(())
}

#[cfg(target_arch = "x86_64")]
fn verify_streaming_elf_pages<F>(
    user_pt: &mut vmm::PageTable,
    elf_info: &elf::ElfInfo,
    read_fn: &F,
    page_size: usize,
) -> Result<(), &'static str>
where
    F: Fn(usize, &mut [u8]) -> Result<usize, ()>,
{
    let mut repaired_count = 0usize;
    let mut first_vaddr = 0usize;
    let mut first_old_pte = None;
    let mut first_old_frame = None;

    for seg_idx in 0..elf_info.num_segments {
        let seg = elf_info.segments[seg_idx].as_ref().unwrap();
        let page_start = seg.vaddr & !(page_size - 1);
        let page_end = (seg.vaddr + seg.mem_size + page_size - 1) & !(page_size - 1);

        for page_addr in (page_start..page_end).step_by(page_size) {
            let old_pte = vmm::debug_pte(user_pt, page_addr);
            let old_frame = vmm::translate_user(user_pt, page_addr).map(|p| p & !(page_size - 1));
            let leaf_user = old_pte
                .map(|raw| vmm::PTEFlags::from_bits_truncate(raw).contains(vmm::PTEFlags::USER))
                .unwrap_or(false);
            let needs_repair = old_frame.map_or(true, |frame| frame == page_addr) || !leaf_user;

            if !needs_repair {
                continue;
            }

            if repaired_count == 0 {
                first_vaddr = page_addr;
                first_old_pte = old_pte;
                first_old_frame = old_frame;
            }
            reload_streaming_elf_page(user_pt, elf_info, read_fn, page_addr, page_size)?;

            let new_pte = vmm::debug_pte(user_pt, page_addr);
            let new_frame = vmm::translate_user(user_pt, page_addr).map(|p| p & !(page_size - 1));
            let repaired_user = new_pte
                .map(|raw| vmm::PTEFlags::from_bits_truncate(raw).contains(vmm::PTEFlags::USER))
                .unwrap_or(false);
            if new_frame.map_or(true, |frame| frame == page_addr) || !repaired_user {
                crate::console_println!(
                    "[ELF-INVARIANT] repair failed vaddr={:#x} old_pte={:?} old_frame={:?} new_pte={:?} new_frame={:?}",
                    page_addr,
                    old_pte,
                    old_frame,
                    new_pte,
                    new_frame
                );
                return Err("ELF: invariant repair failed");
            }
            repaired_count += 1;
        }
    }

    if repaired_count != 0 {
        crate::console_println!(
            "[ELF-INVARIANT] repaired {} PT_LOAD pages first_vaddr={:#x} old_pte={:?} old_frame={:?}",
            repaired_count,
            first_vaddr,
            first_old_pte,
            first_old_frame
        );
    }

    Ok(())
}

impl Process {
    /// Create a new user process from an ELF binary.
    ///
    /// User ELF segments are loaded at their original virtual addresses.
    /// The kernel identity-maps only its own code region (0..4MB), so user
    /// segments starting at 0x400000+ don't conflict.
    pub fn from_elf(
        elf_data: &[u8],
        argv: alloc::vec::Vec<alloc::vec::Vec<u8>>,
        envp: alloc::vec::Vec<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)>,
    ) -> Result<Self, &'static str> {
        // 1. Parse ELF
        let elf = elf::ElfFile::parse(elf_data)?;

        // 2. Create independent user page table
        let user_pt = vmm::create_user_page_table();

        // Guard frame between page table and kernel stack:
        // Timer ISR's TrapContext (184 bytes) can overflow kernel_stack_top by up to
        // 24 bytes. Without this guard, the overflow would corrupt the page table root frame.
        let _guard = pmm::alloc_frame();

        // 3. Allocate kernel stack (needed before copy_kernel_mappings)
        let kernel_stack_top = alloc_kernel_stack().ok_or("Out of memory for kernel stack")?;

        // 4. Copy kernel identity map (2MB huge pages) BEFORE loading ELF.
        // This establishes identity-mapped access to physical memory using the
        // kernel's existing 2MB huge pages. ELF loading can then split these
        // huge pages as needed for 4KB mappings.
        copy_kernel_mappings(user_pt, kernel_stack_top);

        // 4.5 Switch to kernel CR3 for ELF loading.
        // The ELF loader writes frame data via identity-mapped physical addresses
        // (e.g., `write_bytes(frame as *mut u8, 0, 4096)`). This requires a stable
        // identity mapping. But as vmm::map() splits 2MB huge pages in the user
        // page table, the identity mapping can become fragmented/corrupted. Running
        // on the kernel CR3 (which has a complete, untampered identity mapping)
        // ensures all physical frame writes go to the correct destination.
        let saved_cr3: u64;
        #[cfg(target_arch = "x86_64")]
        {
            saved_cr3 = {
                let cr3: u64;
                unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3) };
                cr3
            };
            let kcr3 = crate::mm::vmm::kernel_cr3() as u64;
            if kcr3 != 0 {
                unsafe { core::arch::asm!("mov cr3, {}", in(reg) kcr3) };
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            saved_cr3 = 0;
        }

        // 5. Load ELF segments into user page table. map() will split any
        // 2MB huge pages (PS=1) that overlap with ELF segment addresses.
        let mut max_vaddr = 0usize;
        for segment in &elf.loadable_segments {
            let page_size = pmm::page_size();
            let seg_vaddr_start = segment.vaddr;
            let seg_vaddr_end = segment.vaddr + segment.mem_size;
            let page_start = seg_vaddr_start & !(page_size - 1);
            let page_end = (seg_vaddr_end + page_size - 1) & !(page_size - 1);
            let pte_flags = elf_segment_pte_flags(segment.flags);

            for vaddr in (page_start..page_end).step_by(page_size) {
                // Always allocate a fresh frame for ELF segments.
                // Do NOT reuse identity-mapped frames (from copy_kernel_mappings).
                let frame = if let Some(f) = vmm::translate_user(user_pt, vaddr) {
                    if f == vaddr {
                        // Identity mapping — allocate new frame instead
                        let new_f = pmm::alloc_frame().ok_or("Out of memory for ELF segment")?;
                        vmm::map_user(user_pt, vaddr, new_f, pte_flags);
                        unsafe {
                            core::ptr::write_bytes(new_f as *mut u8, 0, page_size);
                        }
                        new_f
                    } else {
                        // Multiple PT_LOAD segments may share one page. Keep
                        // the physical frame and merge page-level permissions.
                        let merged_flags = merge_page_flags(user_pt, vaddr, pte_flags);
                        vmm::map_user(user_pt, vaddr, f, merged_flags);
                        f
                    }
                } else {
                    let f = pmm::alloc_frame().ok_or("Out of memory for ELF segment")?;
                    vmm::map_user(user_pt, vaddr, f, pte_flags);
                    unsafe {
                        core::ptr::write_bytes(f as *mut u8, 0, page_size);
                    }
                    f
                };

                // Copy segment data that falls within this page
                let seg_data = &elf_data[segment.offset..segment.offset + segment.file_size];
                let copy_start = core::cmp::max(vaddr, seg_vaddr_start);
                let copy_end =
                    core::cmp::min(vaddr + page_size, seg_vaddr_start + segment.file_size);
                if copy_start < copy_end {
                    let src_offset = copy_start - seg_vaddr_start;
                    let dst_offset = copy_start & (page_size - 1);
                    let len = copy_end - copy_start;
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            seg_data[src_offset..src_offset + len].as_ptr(),
                            (frame + dst_offset) as *mut u8,
                            len,
                        );
                    }

                    // NOTE: We used to patch `syscall` (0x0F 0x05) to `int 0x80` (0xCD 0x80)
                    // here. This is NO LONGER needed since we now have proper MSR-based
                    // SYSCALL/SYSRET support. The binary's native `syscall` instruction
                    // works correctly via the LSTAR entry point → dispatch_linux_raw().
                }
            }
            if segment.vaddr + segment.mem_size > max_vaddr {
                max_vaddr = segment.vaddr + segment.mem_size;
            }
        }

        // 5.5 Restore CR3 after ELF segment loading.
        #[cfg(target_arch = "x86_64")]
        if saved_cr3 != 0 {
            unsafe { core::arch::asm!("mov cr3, {}", in(reg) saved_cr3) };
        }

        // 6. Map user stack in user page table (URW, no execute)
        // Map from USER_STACK_TOP downward for USER_STACK_PAGES.
        // The page containing USER_STACK_TOP itself MUST be mapped because
        // Go/C entry code reads [rsp] where rsp = USER_STACK_TOP initially.
        for i in 0..USER_STACK_PAGES {
            let vaddr = (USER_STACK_TOP - i * pmm::page_size()) & !(pmm::page_size() - 1);
            let frame = pmm::alloc_frame().ok_or("Out of memory for user stack")?;
            // Always map, even if identity_map already covered this address
            // (user stack needs URW flags, not kernel-only KRWX)
            vmm::map_user(user_pt, vaddr, frame, vmm::PTEFlags::URW);
        }

        // 6. Build initial stack (same layout as from_elf_streaming)
        let stack_top = USER_STACK_TOP;
        let page_size = pmm::page_size();

        unsafe fn write_u64_to_user(user_pt: &mut vmm::PageTable, addr: usize, val: u64) {
            if let Some(frame) = vmm::translate_user(user_pt, addr) {
                core::ptr::write_volatile(frame as *mut u64, val);
            }
        }
        unsafe fn write_byte_to_user(user_pt: &mut vmm::PageTable, addr: usize, val: u8) {
            if let Some(frame) = vmm::translate_user(user_pt, addr) {
                core::ptr::write_volatile(frame as *mut u8, val);
            }
        }

        // Random bytes at stack_top - 16
        let random_addr = stack_top - 16;
        unsafe {
            write_u64_to_user(user_pt, random_addr, 0x12345678_9ABCDEF0u64);
            write_u64_to_user(user_pt, random_addr + 8, 0xDEADBEEF_FEEDFACEu64);
        }

        // Build envp strings: "KEY=VALUE\0"
        let envp_strs: alloc::vec::Vec<alloc::vec::Vec<u8>> = envp
            .iter()
            .map(|(k, v)| {
                let mut s = alloc::vec::Vec::new();
                s.extend_from_slice(k);
                s.push(b'=');
                s.extend_from_slice(v);
                s.push(0);
                s
            })
            .collect();

        let argv_strs: alloc::vec::Vec<alloc::vec::Vec<u8>> = argv
            .iter()
            .map(|s| {
                let mut v = s.clone();
                if v.last() != Some(&0) {
                    v.push(0);
                }
                v
            })
            .collect();

        let strings_size: usize = argv_strs.iter().map(|s| s.len()).sum::<usize>()
            + envp_strs.iter().map(|s| s.len()).sum::<usize>();

        let strings_start = random_addr - strings_size;

        let mut argv_ptrs: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
        let mut envp_ptrs: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
        let mut str_pos = strings_start;

        for s in &argv_strs {
            argv_ptrs.push(str_pos);
            for &b in s {
                unsafe {
                    write_byte_to_user(user_pt, str_pos, b);
                }
                str_pos += 1;
            }
        }
        for s in &envp_strs {
            envp_ptrs.push(str_pos);
            for &b in s {
                unsafe {
                    write_byte_to_user(user_pt, str_pos, b);
                }
                str_pos += 1;
            }
        }

        // Auxv
        const AT_NULL: usize = 0;
        const AT_PAGESZ: usize = 6;
        const AT_ENTRY: usize = 9;
        const AT_PHDR: usize = 3;
        const AT_PHENT: usize = 4;
        const AT_PHNUM: usize = 5;
        const AT_UID: usize = 11;
        const AT_EUID: usize = 12;
        const AT_GID: usize = 13;
        const AT_EGID: usize = 14;
        const AT_RANDOM: usize = 25;
        const AT_SYSINFO_EHDR: usize = 33;

        let auxv_data: [(usize, usize); 12] = [
            (AT_SYSINFO_EHDR, 0),
            (AT_PHDR, 0),   // from_elf doesn't have phdr vaddr readily available
            (AT_PHENT, 56), // default for 64-bit ELF
            (AT_PHNUM, elf.loadable_segments.len() as usize),
            (AT_PAGESZ, page_size),
            (AT_ENTRY, elf.entry),
            (AT_UID, 0),
            (AT_EUID, 0),
            (AT_GID, 0),
            (AT_EGID, 0),
            (AT_RANDOM, random_addr),
            (AT_NULL, 0),
        ];

        let argc = argv_ptrs.len();
        let metadata_size = 8 + (argc + 1) * 8 + (envp_ptrs.len() + 1) * 8 + auxv_data.len() * 16;
        let metadata_start = (strings_start - metadata_size) & !0xF;

        let mut pos = metadata_start;
        unsafe {
            write_u64_to_user(user_pt, pos, argc as u64);
        }
        pos += 8;
        for &ptr in &argv_ptrs {
            unsafe {
                write_u64_to_user(user_pt, pos, ptr as u64);
            }
            pos += 8;
        }
        unsafe {
            write_u64_to_user(user_pt, pos, 0);
        }
        pos += 8;
        for &ptr in &envp_ptrs {
            unsafe {
                write_u64_to_user(user_pt, pos, ptr as u64);
            }
            pos += 8;
        }
        unsafe {
            write_u64_to_user(user_pt, pos, 0);
        }
        pos += 8;
        for (atype, avalue) in &auxv_data {
            unsafe {
                write_u64_to_user(user_pt, pos, *atype as u64);
            }
            pos += 8;
            unsafe {
                write_u64_to_user(user_pt, pos, *avalue as u64);
            }
            pos += 8;
        }

        let initial_rsp = metadata_start;

        // 7. Store page_table_root as PPN
        let page_table_ppn = (user_pt as *const vmm::PageTable as usize) >> 12;

        // 8. Set up initial brk (after loaded segments, aligned to page)
        let initial_brk = (max_vaddr + page_size - 1) & !(page_size - 1);
        let brk = core::cmp::max(initial_brk, USER_HEAP_BASE);

        // Entry point with relocation offset
        let entry = elf.entry;

        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);

        Ok(Self {
            pid,
            ppid: 0, // Set by caller (kmain sets 0 for init, sys_spawn sets parent pid)
            page_table_root: page_table_ppn,
            kernel_stack_top,
            user_stack_top: initial_rsp,
            brk,
            initial_brk: brk,
            entry: elf.entry,
            state: ProcessState::Ready,
            exit_code: 0,
            fd_table: alloc::sync::Arc::new(spin::Mutex::new(crate::driver::fs::FdTable::new())),
            wait_child_idx: None,
            trap_ctx_ptr: 0,
            shared_page_table: false,
            clone_tls: 0,
            child_tid_ptr: 0,
            fs_base: 0,
            env: BTreeMap::new(),
        })
    }

    /// Create a new user process using streaming ELF loader.
    ///
    /// Unlike `from_elf` which requires the entire ELF data in memory,
    /// this reads only the ELF header + program headers (~4KB) upfront,
    /// then reads each PT_LOAD segment page-by-page via `read_fn`.
    ///
    /// This allows loading large binaries (e.g., 69MB Go executables) from
    /// ext4 without allocating a contiguous 69MB buffer in kernel heap.
    ///
    /// `read_fn(offset, buf)` should fill `buf` with file data starting
    /// at `offset`, returning Ok(bytes_read) or Err on failure.
    ///
    /// `argv` is a list of argument strings (e.g., ["ls", "-l", "/"]).
    /// `envp` is a list of (key, value) environment variable pairs.
    pub fn from_elf_streaming<F>(
        read_fn: F,
        argv: alloc::vec::Vec<alloc::vec::Vec<u8>>,
        envp: alloc::vec::Vec<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)>,
        _kernel_stack_top_hint: usize,
    ) -> Result<Self, &'static str>
    where
        F: Fn(usize, &mut [u8]) -> Result<usize, ()>,
    {
        // Disable interrupts during ELF loading to prevent timer ISR from
        // interfering with page table operations.
        #[cfg(target_arch = "x86_64")]
        x86_64::instructions::interrupts::disable();

        let page_size = pmm::page_size();

        // 0. Allocate kernel stack (needed before copy_kernel_mappings)
        let kernel_stack_top = alloc_kernel_stack().ok_or("Out of memory for kernel stack")?;

        // 0.5 Allocate a guard frame between stack and page table.
        // Timer ISR's TrapContext (184 bytes) can overflow kernel_stack_top by up to
        // 24 bytes. If the page table root frame is adjacent to the stack top,
        // this corruption destroys PML4 entries. The guard frame prevents this.
        let _guard = pmm::alloc_frame();

        // 1. Read ELF header (first 4096 bytes covers header + program headers)
        let header_size = 4096usize;
        let mut header_buf = alloc::vec![0u8; header_size];
        let bytes_read = read_fn(0, &mut header_buf).map_err(|_| "ELF: failed to read header")?;
        if bytes_read < core::mem::size_of::<elf::ElfHeader>() {
            return Err("ELF: header too small");
        }

        // 2. Parse header and segment info
        let elf_info = elf::ElfInfo::parse_header_only(&header_buf[..bytes_read])?;

        // 3. Create independent user page table
        let user_pt = vmm::create_user_page_table();

        // Guard frame between page table and kernel stack:
        // Timer ISR's TrapContext (184 bytes) can overflow kernel_stack_top by up to
        // 24 bytes. Without this guard, the overflow would corrupt the page table root frame.
        let _guard = pmm::alloc_frame();

        // 4. Copy kernel identity map (2MB huge pages) BEFORE loading ELF.
        // This establishes identity-mapped access to physical memory using the
        // kernel's existing 2MB huge pages. ELF loading can then split these
        // huge pages as needed for 4KB mappings.
        copy_kernel_mappings(user_pt, kernel_stack_top);

        // 4.5 Switch to kernel CR3 for ELF loading.
        // The ELF loader writes frame data via identity-mapped physical addresses
        // (e.g., `write_bytes(frame as *mut u8, 0, 4096)`). This requires a stable
        // identity mapping. But as vmm::map() splits 2MB huge pages in the user
        // page table, the identity mapping can become fragmented/corrupted. Running
        // on the kernel CR3 (which has a complete, untampered identity mapping)
        // ensures all physical frame writes go to the correct destination.
        let saved_cr3: u64;
        #[cfg(target_arch = "x86_64")]
        {
            saved_cr3 = {
                let cr3: u64;
                unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3) };
                cr3
            };
            let kcr3 = crate::mm::vmm::kernel_cr3() as u64;
            if kcr3 != 0 {
                unsafe { core::arch::asm!("mov cr3, {}", in(reg) kcr3) };
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            saved_cr3 = 0;
        }

        // 5. Load ELF segments page-by-page. map() will split any 2MB huge
        // pages (PS=1) that overlap with ELF segment addresses.
        let mut max_vaddr = 0usize;
        let mut total_pages: usize = 0;
        // Collect segment ranges for VMA registration (prevents mmap overlap)
        let mut elf_segments_vma: alloc::vec::Vec<(usize, usize, usize)> = alloc::vec::Vec::new();
        for seg_idx in 0..elf_info.num_segments {
            let seg = elf_info.segments[seg_idx].as_ref().unwrap();
            let seg_vaddr_start = seg.vaddr;
            let seg_vaddr_end = seg.vaddr + seg.mem_size;
            let page_start = seg_vaddr_start & !(page_size - 1);
            let page_end = (seg_vaddr_end + page_size - 1) & !(page_size - 1);
            let pte_flags = elf_segment_pte_flags(seg.flags as usize);
            let num_pages = (page_end - page_start) / page_size;

            // Register this segment for VMA tracking (to prevent mmap overlap)
            // Convert ELF flags to Linux PROT flags for VMA registration
            let seg_prot = {
                let mut p = 0usize;
                if seg.flags & 1 != 0 {
                    p |= 4;
                } // PF_X → PROT_EXEC
                if seg.flags & 2 != 0 {
                    p |= 2;
                } // PF_W → PROT_WRITE
                if seg.flags & 4 != 0 {
                    p |= 1;
                } // PF_R → PROT_READ
                p
            };
            elf_segments_vma.push((page_start, page_end, seg_prot));

            for vaddr in (page_start..page_end).step_by(page_size) {
                // Always allocate a fresh frame for ELF segments.
                // Do NOT reuse identity-mapped frames (from copy_kernel_mappings),
                // as their physical addresses may collide with critical structures
                // like the current CR3 page table root.
                let frame = if let Some(f) = vmm::translate_user(user_pt, vaddr) {
                    // Check if this is an identity mapping (vaddr == paddr).
                    // Identity-mapped pages come from copy_kernel_mappings and
                    // their frames should NOT be reused for ELF segments — they
                    // contain page table structures or other critical data.
                    if f == vaddr {
                        // Allocate a new frame and overwrite the identity mapping
                        let new_f = pmm::alloc_frame().ok_or("Out of memory for ELF segment")?;
                        vmm::map_user(user_pt, vaddr, new_f, pte_flags);
                        unsafe {
                            core::ptr::write_bytes(new_f as *mut u8, 0, page_size);
                        }
                        new_f
                    } else {
                        // Non-identity mapping from a previous segment — reuse it
                        let merged_flags = merge_page_flags(user_pt, vaddr, pte_flags);
                        vmm::map_user(user_pt, vaddr, f, merged_flags);
                        f
                    }
                } else {
                    // No mapping exists — allocate a new frame
                    let f = pmm::alloc_frame().ok_or("Out of memory for ELF segment")?;
                    vmm::map_user(user_pt, vaddr, f, pte_flags);
                    // Zero-fill the entire frame
                    unsafe {
                        core::ptr::write_bytes(f as *mut u8, 0, page_size);
                    }
                    f
                };

                total_pages += 1;

                // Determine what portion of this page comes from the file
                let copy_start = core::cmp::max(vaddr, seg_vaddr_start);
                let copy_end = core::cmp::min(vaddr + page_size, seg_vaddr_start + seg.file_size);
                if copy_start < copy_end {
                    let file_offset = seg.offset + (copy_start - seg_vaddr_start);
                    let dst_offset = copy_start & (page_size - 1);
                    let len = copy_end - copy_start;
                    // Read file data into a temporary buffer, then copy to frame
                    let mut tmp_buf = [0u8; 4096];
                    let bytes = read_fn(file_offset, &mut tmp_buf[..len])
                        .map_err(|_| "ELF: failed to read segment data")?;
                    if bytes < len {
                        // Short read — zero-fill the rest
                        unsafe {
                            core::ptr::write_bytes(tmp_buf[bytes..].as_mut_ptr(), 0, len - bytes);
                        }
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            tmp_buf[..len].as_ptr(),
                            (frame + dst_offset) as *mut u8,
                            len,
                        );
                    }

                    // NOTE: No longer patching syscall→int 0x80. MSR SYSCALL/SYSRET
                    // is properly configured. The binary's native `syscall` instruction
                    // works via LSTAR → dispatch_linux_raw().
                }
            }
            if seg.vaddr + seg.mem_size > max_vaddr {
                max_vaddr = seg.vaddr + seg.mem_size;
            }
        } // end for seg_idx

        // 5.05 Enforce ELF mapping invariants before any user code can run:
        // every PT_LOAD page must be user-accessible and backed by a private
        // non-identity frame. If a copied identity mapping leaked through,
        // rebuild that page from the ELF file immediately.
        #[cfg(target_arch = "x86_64")]
        {
            verify_streaming_elf_pages(user_pt, &elf_info, &read_fn, page_size)?;
        }

        // 5.1 Register ELF segments as VMA entries to prevent mmap from
        // allocating addresses that overlap with loaded ELF data.
        // Also ensure the mmap bump allocator starts past all ELF segments.
        for &(start, end, prot) in &elf_segments_vma {
            crate::syscall::register_elf_vma(start, end, prot);
        }
        if max_vaddr > 0 {
            crate::syscall::ensure_mmap_above(max_vaddr);
        }

        // 5.5 Restore CR3 after ELF segment loading.
        // All frame writes (via identity-mapped physical addresses) are done.
        // Restore the original CR3 so subsequent page table setup works correctly.
        #[cfg(target_arch = "x86_64")]
        if saved_cr3 != 0 {
            unsafe { core::arch::asm!("mov cr3, {}", in(reg) saved_cr3) };
        }

        // 6. Map user stack in user page table (URW, no execute)
        // Map from USER_STACK_TOP downward for USER_STACK_PAGES.
        // Include the page containing USER_STACK_TOP itself (Go/C reads [rsp] on entry).
        for i in 0..USER_STACK_PAGES {
            let frame = pmm::alloc_frame().ok_or("Out of memory for user stack")?;
            let vaddr = (USER_STACK_TOP - i * pmm::page_size()) & !(pmm::page_size() - 1);
            vmm::map_user(user_pt, vaddr, frame, vmm::PTEFlags::URW);
        }

        // 7. Set up initial stack with Linux execve layout:
        //
        // High addresses (near USER_STACK_TOP):
        //   random bytes (16B)
        //   strings area (argv strings + envp strings, each \0 terminated)
        //   padding (align to 16)
        //   auxv entries (16B each) + AT_NULL
        //   envp NULL terminator
        //   envp[n-1] ... envp[0] (8B pointers to strings)
        //   argv NULL terminator
        //   argv[argc-1] ... argv[0] (8B pointers to strings)
        //   argc (8B) ← RSP points here
        // Low addresses

        const AT_NULL: usize = 0;
        const AT_PAGESZ: usize = 6;
        const AT_ENTRY: usize = 9;
        const AT_PHDR: usize = 3;
        const AT_PHENT: usize = 4;
        const AT_PHNUM: usize = 5;
        const AT_UID: usize = 11;
        const AT_EUID: usize = 12;
        const AT_GID: usize = 13;
        const AT_EGID: usize = 14;
        const AT_RANDOM: usize = 25;
        const AT_SYSINFO_EHDR: usize = 33; // VDSO

        let stack_top = USER_STACK_TOP;
        let phdr_addr = elf_info.phdr_vaddr;

        // Helper to write u64 to user stack via physical address
        unsafe fn write_u64_to_user(user_pt: &mut vmm::PageTable, addr: usize, val: u64) {
            if let Some(frame) = vmm::translate_user(user_pt, addr) {
                core::ptr::write_volatile(frame as *mut u64, val);
            }
        }

        // Helper to write a byte to user stack
        unsafe fn write_byte_to_user(user_pt: &mut vmm::PageTable, addr: usize, val: u8) {
            if let Some(frame) = vmm::translate_user(user_pt, addr) {
                core::ptr::write_volatile(frame as *mut u8, val);
            }
        }

        // Random bytes pointed to by AT_RANDOM.
        let random_addr = stack_top - 16;
        unsafe {
            write_u64_to_user(user_pt, random_addr, 0x12345678_9ABCDEF0u64);
            write_u64_to_user(user_pt, random_addr + 8, 0xDEADBEEF_FEEDFACEu64);
        }

        // Build and copy argv/envp strings above the metadata area.  Linux
        // userspace, including Go, expects argc/argv/envp to be present even
        // when the binary was loaded through the streaming path.
        let envp_strs: alloc::vec::Vec<alloc::vec::Vec<u8>> = envp
            .iter()
            .map(|(k, v)| {
                let mut s = alloc::vec::Vec::new();
                s.extend_from_slice(k);
                s.push(b'=');
                s.extend_from_slice(v);
                s.push(0);
                s
            })
            .collect();

        let argv_strs: alloc::vec::Vec<alloc::vec::Vec<u8>> = argv
            .iter()
            .map(|s| {
                let mut v = s.clone();
                if v.last() != Some(&0) {
                    v.push(0);
                }
                v
            })
            .collect();

        let strings_size: usize = argv_strs.iter().map(|s| s.len()).sum::<usize>()
            + envp_strs.iter().map(|s| s.len()).sum::<usize>();
        let strings_start = random_addr - strings_size;

        let mut argv_ptrs: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
        let mut envp_ptrs: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
        let mut str_pos = strings_start;

        for s in &argv_strs {
            argv_ptrs.push(str_pos);
            for &b in s {
                unsafe {
                    write_byte_to_user(user_pt, str_pos, b);
                }
                str_pos += 1;
            }
        }
        for s in &envp_strs {
            envp_ptrs.push(str_pos);
            for &b in s {
                unsafe {
                    write_byte_to_user(user_pt, str_pos, b);
                }
                str_pos += 1;
            }
        }

        let auxv_data: [(usize, usize); 12] = [
            (AT_SYSINFO_EHDR, 0),
            (AT_PHDR, phdr_addr),
            (AT_PHENT, elf_info.phent),
            (AT_PHNUM, elf_info.phnum as usize),
            (AT_PAGESZ, page_size),
            (AT_ENTRY, elf_info.entry),
            (AT_UID, 0),
            (AT_EUID, 0),
            (AT_GID, 0),
            (AT_EGID, 0),
            (AT_RANDOM, random_addr),
            (AT_NULL, 0),
        ];

        let argc = argv_ptrs.len();
        let metadata_size = 8 + (argc + 1) * 8 + (envp_ptrs.len() + 1) * 8 + auxv_data.len() * 16;
        let initial_rsp = (strings_start - metadata_size) & !0xF;

        let mut pos = initial_rsp;
        unsafe {
            write_u64_to_user(user_pt, pos, argc as u64);
        }
        pos += 8;
        for &ptr in &argv_ptrs {
            unsafe {
                write_u64_to_user(user_pt, pos, ptr as u64);
            }
            pos += 8;
        }
        unsafe {
            write_u64_to_user(user_pt, pos, 0);
        }
        pos += 8;
        for &ptr in &envp_ptrs {
            unsafe {
                write_u64_to_user(user_pt, pos, ptr as u64);
            }
            pos += 8;
        }
        unsafe {
            write_u64_to_user(user_pt, pos, 0);
        }
        pos += 8;
        for (atype, avalue) in &auxv_data {
            unsafe {
                write_u64_to_user(user_pt, pos, *atype as u64);
            }
            pos += 8;
            unsafe {
                write_u64_to_user(user_pt, pos, *avalue as u64);
            }
            pos += 8;
        }

        // 8. Store page_table_root as PPN
        let page_table_ppn = (user_pt as *const vmm::PageTable as usize) >> 12;

        // 9. Set up initial brk (after loaded segments, aligned to page)
        let initial_brk = (max_vaddr + page_size - 1) & !(page_size - 1);
        let brk = core::cmp::max(initial_brk, USER_HEAP_BASE);

        // Re-enable interrupts after ELF loading completes
        #[cfg(target_arch = "x86_64")]
        x86_64::instructions::interrupts::enable();

        // ── Critical: restore kernel stack identity mappings in user page table ──
        // ELF loading may have overwritten the identity-mapped pages covering the
        // kernel stack area. Since trap_return_user runs on this stack after switching
        // to the user page table (CR3), the stack MUST point to the original physical
        // frames, not ELF data frames.
        #[cfg(target_arch = "x86_64")]
        {
            map_kernel_stack_pages(user_pt, kernel_stack_top);
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = _kernel_stack_top_hint;

        let entry = elf_info.entry;

        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);

        Ok(Self {
            pid,
            ppid: 0, // Set by caller
            page_table_root: page_table_ppn,
            kernel_stack_top,
            user_stack_top: initial_rsp,
            brk,
            initial_brk: brk,
            entry,
            state: ProcessState::Ready,
            exit_code: 0,
            fd_table: alloc::sync::Arc::new(spin::Mutex::new(crate::driver::fs::FdTable::new())),
            wait_child_idx: None,
            trap_ctx_ptr: 0,
            shared_page_table: false,
            clone_tls: 0,
            child_tid_ptr: 0,
            fs_base: 0,
            env: BTreeMap::new(),
        })
    }
}

/// Per-CPU storage for the current trap context pointer (x86_64 only).
/// Set by trap_handler before calling dispatch, so linux_clone can read
/// the parent's full register state to build the child's TrapContext.
#[cfg(target_arch = "x86_64")]
static CURRENT_TRAP_CTX: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];

#[cfg(target_arch = "x86_64")]
/// Set the current trap context pointer (called from trap_handler).
pub fn set_trap_ctx_ptr(ptr: usize) {
    CURRENT_TRAP_CTX[hartid()].store(ptr, Ordering::Relaxed);
}

#[cfg(target_arch = "x86_64")]
/// Get the current trap context pointer (called from linux_clone).
pub fn get_trap_ctx_ptr() -> usize {
    CURRENT_TRAP_CTX[hartid()].load(Ordering::Relaxed)
}

/// Get a mutable reference to the user page table from its PPN
pub fn get_user_page_table(ppn: usize) -> &'static mut vmm::PageTable {
    unsafe { &mut *((ppn << 12) as *mut vmm::PageTable) }
}

// ─── Global process table ─────────────────────────────────────────

/// Global process list (simplified for Phase 2)
static PROCESS_TABLE: Mutex<[Option<Process>; MAX_PROCESSES]> =
    Mutex::new([const { None }; MAX_PROCESSES]);

/// Current running process index — per-hart array for SMP
static CURRENT_PROCESS: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];

/// Current process's page table root PPN (lock-free for trap handler access) — per-hart
static CURRENT_PAGE_TABLE_ROOT: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];

fn hartid() -> usize {
    let h = crate::arch::smp::current_hart();
    if h >= 8 { 0 } else { h }
}

/// Get the current process (cloned, acquires lock).
/// Do NOT call from trap handler — use `current_page_table_root()` instead.
pub fn current() -> Option<Process> {
    let table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
    table[idx].clone()
}

/// Get the current process's page table root PPN.
/// Safe to call from trap handler — uses AtomicUsize, no lock needed.
pub fn current_page_table_root() -> usize {
    CURRENT_PAGE_TABLE_ROOT[hartid()].load(Ordering::Relaxed)
}

/// Update the current page table root (called during process switch).
pub fn set_current_page_table_root(root: usize) {
    CURRENT_PAGE_TABLE_ROOT[hartid()].store(root, Ordering::Relaxed);
}

/// Update the current process
pub fn update_current<F, R>(f: F) -> R
where
    F: FnOnce(&mut Option<Process>) -> R,
{
    let mut table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
    f(&mut table[idx])
}

/// Add a process to the table, returns its index
pub fn add_process(proc: Process) -> Option<usize> {
    let mut table = PROCESS_TABLE.lock();
    for (i, slot) in table.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(proc);
            return Some(i);
        }
    }
    None
}

/// Get page table root for a specific process index (for scheduler use).
/// Acquires lock — do NOT call from trap handler.
pub fn get_page_table_root(idx: usize) -> usize {
    let table = PROCESS_TABLE.lock();
    table[idx].as_ref().map(|p| p.page_table_root).unwrap_or(0)
}

/// Set the current process index
pub fn set_current_index(idx: usize) {
    CURRENT_PROCESS[hartid()].store(idx, Ordering::Relaxed);
}

/// Get current process brk
pub fn current_brk() -> usize {
    let table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
    table[idx].as_ref().map(|p| p.brk).unwrap_or(USER_HEAP_BASE)
}

/// Set current process brk
pub fn set_current_brk(addr: usize) {
    let mut table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
    if let Some(p) = table[idx].as_mut() {
        p.brk = addr;
    }
}

/// Get current process page table root (PPN)
pub fn current_page_table_ppn() -> usize {
    let table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
    table[idx].as_ref().map(|p| p.page_table_root).unwrap_or(0)
}

/// Set exit code for the current process and mark it Exited.
/// Called from sys_exit before schedule_exit(). Marking the state here is what
/// lets a waiting parent observe the child's exit via get_exit_code()/waitpid.
pub fn set_exit_code(code: usize) {
    let mut table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
    if let Some(p) = table[idx].as_mut() {
        p.exit_code = code;
        p.state = ProcessState::Exited;
    }
}

/// Free a process table slot so it can be reused by new clone/fork calls.
/// Called from sys_exit() after marking the process as Exited but before
/// schedule_exit(). The scheduler slot (TaskControlBlock) is NOT freed here.
pub fn free_process_slot(idx: usize) {
    let mut table = PROCESS_TABLE.lock();
    table[idx] = None;
}

/// Set exit code for a process by its table index and mark it exited.
/// This is what makes a polling parent observe completion via waitpid().
pub fn set_exit_code_by_index(idx: usize, code: usize) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table[idx].as_mut() {
        p.exit_code = code;
        p.state = ProcessState::Exited;
    }
}

/// Set child_tid_ptr for the current process.
pub fn set_child_tid_ptr(tidptr: usize) {
    let mut table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
    if let Some(p) = table[idx].as_mut() {
        p.child_tid_ptr = tidptr;
    }
}

/// Get process state by process index.
/// Returns None if process doesn't exist.
pub fn get_state(idx: usize) -> Option<ProcessState> {
    let table = PROCESS_TABLE.lock();
    table[idx].as_ref().map(|p| p.state)
}

/// Get exit code of a process by index.
/// Returns None if process doesn't exist or hasn't exited.
pub fn get_exit_code(idx: usize) -> Option<usize> {
    let table = PROCESS_TABLE.lock();
    table[idx].as_ref().and_then(|p| {
        if p.state == ProcessState::Exited {
            Some(p.exit_code)
        } else {
            None
        }
    })
}

/// Find a child process of the given parent that has exited.
/// Returns (process_index, exit_code) or None.
pub fn find_exited_child(parent_pid: usize) -> Option<(usize, usize)> {
    let table = PROCESS_TABLE.lock();
    for (idx, proc) in table.iter().enumerate() {
        if let Some(p) = proc {
            if p.ppid == parent_pid && p.state == ProcessState::Exited {
                return Some((idx, p.exit_code));
            }
        }
    }
    None
}

/// Check if the given process has any children.
pub fn has_children(parent_pid: usize) -> bool {
    let table = PROCESS_TABLE.lock();
    table
        .iter()
        .any(|p| p.as_ref().map_or(false, |proc| proc.ppid == parent_pid))
}

/// Reclaim a process's resources and remove it from the table.
/// Returns true if successful.
/// Acquires lock — do NOT call from trap handler.
pub fn reclaim_process(idx: usize) -> bool {
    let proc = {
        let mut table = PROCESS_TABLE.lock();
        table[idx].take()
    };

    if let Some(p) = proc {
        // Free user page table (all user-mapped frames + page table frames)
        // Skip if page table is shared (CLONE_VM child) — parent owns it
        if p.page_table_root != 0 && !p.shared_page_table {
            crate::mm::vmm::free_user_page_table(p.page_table_root);
        }
        dealloc_kernel_stack(p.kernel_stack_top);
        true
    } else {
        false
    }
}

/// Get parent pid of current process.
pub fn current_ppid() -> usize {
    let table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
    table[idx].as_ref().map(|p| p.ppid).unwrap_or(0)
}

/// Set ppid for a process by index.
pub fn set_ppid(idx: usize, ppid: usize) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table[idx].as_mut() {
        p.ppid = ppid;
    }
}

/// Get current process pid.
pub fn current_pid() -> usize {
    let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
    let table = PROCESS_TABLE.lock();
    table
        .get(idx)
        .and_then(|p| p.as_ref())
        .map(|p| p.pid)
        .unwrap_or(0)
}

/// Get the current process's FD table as a mutable reference.
/// This is used by syscall handlers for file operations.
///
/// # Safety
/// Caller must ensure no nested lock acquisition on PROCESS_TABLE.
#[cfg(not(feature = "test_mode"))]
pub fn with_fd_table<F, R>(f: F) -> R
where
    F: FnOnce(&mut crate::driver::fs::FdTable) -> R,
{
    // Get Arc reference under PROCESS_TABLE lock
    let fd_arc = {
        let table = PROCESS_TABLE.lock();
        let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
        let proc = table[idx].as_ref().expect("No current process");
        proc.fd_table.clone() // Arc::clone — cheap refcount bump
    }; // PROCESS_TABLE dropped here

    let mut fd_table = fd_arc.lock();
    f(&mut fd_table)
}

/// Test mode fallback — uses a global static FD table.
#[cfg(feature = "test_mode")]
use crate::sync::spinlock::SpinLock;

#[cfg(feature = "test_mode")]
static TEST_FD_TABLE: SpinLock<Option<crate::driver::fs::FdTable>> = SpinLock::new(None);

#[cfg(feature = "test_mode")]
pub fn with_fd_table<F, R>(f: F) -> R
where
    F: FnOnce(&mut crate::driver::fs::FdTable) -> R,
{
    let mut guard = TEST_FD_TABLE.lock();
    if guard.is_none() {
        *guard = Some(crate::driver::fs::FdTable::new());
    }
    f(guard.as_mut().unwrap())
}

/// Find process index by PID.
pub fn find_process_by_pid(pid: usize) -> Option<usize> {
    let table = PROCESS_TABLE.lock();
    for (idx, p) in table.iter().enumerate() {
        if p.as_ref().map_or(false, |proc| proc.pid == pid) {
            return Some(idx);
        }
    }
    None
}

/// Get ppid of a process by index.
pub fn get_ppid(idx: usize) -> usize {
    let table = PROCESS_TABLE.lock();
    table[idx].as_ref().map(|p| p.ppid).unwrap_or(0)
}

/// Get current process index.
pub fn current_index() -> usize {
    CURRENT_PROCESS[hartid()].load(Ordering::Relaxed)
}

/// Get the kernel stack top for a process (used for TSS.RSP0 on x86_64).
/// Returns None if the process doesn't exist or has no kernel stack.
pub fn get_kernel_sp(proc_idx: usize) -> Option<usize> {
    let table = PROCESS_TABLE.lock();
    table[proc_idx].as_ref().map(|p| p.kernel_stack_top)
}

/// Set FS_BASE MSR value for a process (used for TLS restore on context switch).
#[cfg(target_arch = "x86_64")]
pub fn set_fs_base(proc_idx: usize, val: u64) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(ref mut p) = table[proc_idx] {
        p.fs_base = val;
    }
}

/// Get FS_BASE MSR value for a process.
#[cfg(target_arch = "x86_64")]
pub fn get_fs_base(proc_idx: usize) -> u64 {
    let table = PROCESS_TABLE.lock();
    table[proc_idx].as_ref().map(|p| p.fs_base).unwrap_or(0)
}

/// Set wait_child_idx for a process (marks it as waiting for a specific child).
pub fn set_wait_child(proc_idx: usize, child_idx: Option<usize>) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table[proc_idx].as_mut() {
        p.wait_child_idx = child_idx;
    }
}

/// Find the parent process that is waiting for a child at the given index.
/// Returns the parent's process index, or None.
pub fn find_waiting_parent(child_idx: usize) -> Option<usize> {
    let table = PROCESS_TABLE.lock();
    for (idx, p) in table.iter().enumerate() {
        if let Some(proc) = p {
            if proc.wait_child_idx == Some(child_idx) {
                return Some(idx);
            }
        }
    }
    None
}

/// Get process index by process index (validation).
/// Returns true if the process at this index exists and has the given ppid.
pub fn is_child_of(proc_idx: usize, parent_pid: usize) -> bool {
    let table = PROCESS_TABLE.lock();
    table[proc_idx]
        .as_ref()
        .map_or(false, |p| p.ppid == parent_pid)
}

/// Set process state by index (used by sys_kill).
pub fn set_state(idx: usize, state: ProcessState) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table[idx].as_mut() {
        p.state = state;
    }
}

/// Get a cloned process by its table index.
pub fn get_process_by_index(idx: usize) -> Option<Process> {
    let table = PROCESS_TABLE.lock();
    table[idx].clone()
}

/// Return all process table indices that share the same page table root.
pub fn find_processes_by_page_table_root(root: usize) -> alloc::vec::Vec<usize> {
    let table = PROCESS_TABLE.lock();
    let mut indices = alloc::vec::Vec::new();
    for (idx, proc_opt) in table.iter().enumerate() {
        if let Some(proc) = proc_opt {
            if proc.page_table_root == root {
                indices.push(idx);
            }
        }
    }
    indices
}

/// Return the process table index for the thread-group leader of an address space.
pub fn find_group_leader_by_page_table_root(root: usize) -> Option<usize> {
    let table = PROCESS_TABLE.lock();
    let mut fallback = None;
    for (idx, proc_opt) in table.iter().enumerate() {
        if let Some(proc) = proc_opt {
            if proc.page_table_root == root {
                fallback.get_or_insert(idx);
                if !proc.shared_page_table {
                    return Some(idx);
                }
            }
        }
    }
    fallback
}

/// Kill all clone child threads of the given PID.
/// Used when a thread group leader exits (exit_group) to terminate
/// all CLONE_THREAD children that share the same address space.
pub fn kill_clone_children(parent_pid: usize) {
    let mut indices_to_kill: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    {
        let table = PROCESS_TABLE.lock();
        // Collect ALL descendants of parent_pid (children, grandchildren, etc.)
        // Go's CLONE_THREAD creates threads where child's ppid = caller's pid,
        // so thread groups can be multi-level: PID=2 → PID=4 → PID=5,6,7
        let mut frontier: alloc::vec::Vec<usize> = alloc::vec![parent_pid];
        while let Some(ppid) = frontier.pop() {
            for (i, proc_opt) in table.iter().enumerate() {
                if indices_to_kill.contains(&i) {
                    continue;
                }
                if let Some(p) = proc_opt {
                    if p.ppid == ppid && p.pid != parent_pid {
                        indices_to_kill.push(i);
                        frontier.push(p.pid);
                    }
                }
            }
        }
    }
    crate::syscall::cleanup_futex_waiters_for_processes(&indices_to_kill);

    for idx in indices_to_kill {
        // CLONE_CHILD_CLEARTID: notify futex waiters
        {
            let table = PROCESS_TABLE.lock();
            if let Some(p) = table[idx].as_ref() {
                if p.child_tid_ptr != 0 {
                    let tid_ptr = p.child_tid_ptr;
                    drop(table);
                    crate::syscall::user_write::<i32>(tid_ptr, 0);
                    // Wake futex waiters
                    crate::syscall::linux_futex(tid_ptr, 1, 1);
                }
            }
        }
        crate::process::set_state(idx, ProcessState::Exited);
        crate::process::set_exit_code_by_index(idx, 0);
        // Mark the scheduler task as Exited so it won't be scheduled again.
        // This prevents the timer ISR from resuming a thread whose pages
        // may have been freed when the thread group leader exited.
        crate::sched::mark_task_exited_by_proc(idx);
        // Free the process table slot so it can be reused
        crate::process::free_process_slot(idx);
    }
    // On SMP, other cores may still be running clone child threads.
    // Send IPI to force immediate reschedule on all other cores.
    #[cfg(target_arch = "x86_64")]
    crate::arch::lapic::broadcast_reschedule();
}

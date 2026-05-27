//! Process management: user-space processes with independent address spaces.

pub mod elf;

use core::sync::atomic::{AtomicUsize, Ordering};

use spin::Mutex;

use crate::mm::{pmm, vmm};

/// User address space layout constants
pub const USER_CODE_BASE: usize = 0x0000_0000;
pub const USER_CODE_LIMIT: usize = 0x0040_0000; // 4 MB for code + data
pub const USER_HEAP_BASE: usize = 0x0040_0000;
pub const USER_HEAP_LIMIT: usize = 0x0080_0000; // 4 MB heap
pub const USER_STACK_TOP: usize = 0x8000_0000; // Top of user stack
pub const USER_STACK_BASE: usize = 0x7FC0_0000; // 4 MB stack
pub const USER_STACK_PAGES: usize = 64; // 256 KB actual stack

/// Process identifier allocator
static NEXT_PID: AtomicUsize = AtomicUsize::new(1);

/// Maximum number of processes in the system
const MAX_PROCESSES: usize = 16;

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
    /// Root page table PPN (physical page number of the Sv39 root page table)
    pub page_table_root: usize,
    /// Kernel stack top (used for trap handling when in U-mode)
    pub kernel_stack_top: usize,
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
    /// Trap context pointer (saved on kernel stack)
    pub trap_ctx_ptr: usize,
}

/// Copy kernel identity mappings into a user page table.
/// This is needed so that traps from U-mode can still access kernel code/data.
fn copy_kernel_mappings(user_pt: &mut vmm::PageTable) {
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

impl Process {
    /// Create a new user process from an ELF binary embedded in the kernel.
    /// `elf_data` is the raw ELF file bytes (statically linked into the kernel).
    ///
    /// Creates an independent page table for the process with:
    /// - Kernel identity mappings (so traps can access kernel code/data)
    /// - User code/data segments (URWX)
    /// - User stack (URW)
    pub fn from_elf(elf_data: &[u8]) -> Result<Self, &'static str> {
        // 1. Parse ELF
        let elf = elf::ElfFile::parse(elf_data)?;

        // 2. Create independent user page table
        let user_pt = vmm::create_user_page_table();

        // 3. Copy kernel mappings so traps can access kernel code
        copy_kernel_mappings(user_pt);

        // 4. Load ELF segments into user page table
        let mut max_vaddr = 0usize;
        for segment in &elf.loadable_segments {
            let page_size = pmm::page_size();
            let start_page = segment.vaddr & !(page_size - 1);
            let end_page = (segment.vaddr + segment.mem_size + page_size - 1) & !(page_size - 1);

            for page_start in (start_page..end_page).step_by(page_size) {
                let frame = pmm::alloc_frame().ok_or("Out of memory for ELF segment")?;

                // Map in user page table with URWX flags
                vmm::map_user(user_pt, page_start, frame, vmm::PTEFlags::URWX);

                // Zero the page
                unsafe {
                    core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                }

                // Copy segment data into this page
                let page_offset_in_seg = page_start - segment.vaddr;
                let seg_data = &elf_data[segment.offset..segment.offset + segment.file_size];
                let src_start = page_offset_in_seg;
                let src_end = core::cmp::min(src_start + page_size, seg_data.len());
                if src_start < src_end {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            seg_data[src_start..src_end].as_ptr(),
                            (frame + src_start) as *mut u8,
                            src_end - src_start,
                        );
                    }
                }
            }

            if segment.vaddr + segment.mem_size > max_vaddr {
                max_vaddr = segment.vaddr + segment.mem_size;
            }
        }

        // 5. Map user stack in user page table (URW, no execute)
        for i in 0..USER_STACK_PAGES {
            let frame = pmm::alloc_frame().ok_or("Out of memory for user stack")?;
            let vaddr = USER_STACK_TOP - (i + 1) * pmm::page_size();
            vmm::map_user(user_pt, vaddr, frame, vmm::PTEFlags::URW);
        }

        // 6. Allocate kernel stack for this process (identity mapped already by vmm::init)
        let kstack_base = pmm::alloc_frame().ok_or("Out of memory for kernel stack")?;
        for _ in 0..3 {
            pmm::alloc_frame().ok_or("Out of memory for kernel stack")?;
        }
        let kernel_stack_top = kstack_base + 4 * pmm::page_size();

        // 7. Store page_table_root as PPN
        let page_table_ppn = (user_pt as *const vmm::PageTable as usize) >> 12;

        // 8. Set up initial brk
        let page_size = pmm::page_size();
        let initial_brk = (max_vaddr + page_size - 1) & !(page_size - 1);
        let brk = core::cmp::max(initial_brk, USER_HEAP_BASE);

        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);

        Ok(Self {
            pid,
            page_table_root: page_table_ppn,
            kernel_stack_top,
            user_stack_top: USER_STACK_TOP,
            brk,
            initial_brk: brk,
            entry: elf.entry,
            state: ProcessState::Ready,
            trap_ctx_ptr: 0,
        })
    }
}

/// Get a mutable reference to the user page table from its PPN
pub fn get_user_page_table(ppn: usize) -> &'static mut vmm::PageTable {
    unsafe { &mut *((ppn << 12) as *mut vmm::PageTable) }
}

// ─── Global process table ─────────────────────────────────────────

/// Global process list (simplified for Phase 2)
static PROCESS_TABLE: Mutex<[Option<Process>; MAX_PROCESSES]> =
    Mutex::new([const { None }; MAX_PROCESSES]);

/// Current running process index
static CURRENT_PROCESS: AtomicUsize = AtomicUsize::new(0);

/// Current process's page table root PPN (lock-free for trap handler access)
static CURRENT_PAGE_TABLE_ROOT: AtomicUsize = AtomicUsize::new(0);

/// Get the current process (cloned, acquires lock).
/// Do NOT call from trap handler — use `current_page_table_root()` instead.
pub fn current() -> Option<Process> {
    let table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS.load(Ordering::Relaxed);
    table[idx].clone()
}

/// Get the current process's page table root PPN.
/// Safe to call from trap handler — uses AtomicUsize, no lock needed.
pub fn current_page_table_root() -> usize {
    CURRENT_PAGE_TABLE_ROOT.load(Ordering::Relaxed)
}

/// Update the current page table root (called during process switch).
pub fn set_current_page_table_root(root: usize) {
    CURRENT_PAGE_TABLE_ROOT.store(root, Ordering::Relaxed);
}

/// Update the current process
pub fn update_current<F, R>(f: F) -> R
where
    F: FnOnce(&mut Option<Process>) -> R,
{
    let mut table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS.load(Ordering::Relaxed);
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
    CURRENT_PROCESS.store(idx, Ordering::Relaxed);
}

/// Get current process brk
pub fn current_brk() -> usize {
    let table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS.load(Ordering::Relaxed);
    table[idx].as_ref().map(|p| p.brk).unwrap_or(USER_HEAP_BASE)
}

/// Set current process brk
pub fn set_current_brk(addr: usize) {
    let mut table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS.load(Ordering::Relaxed);
    if let Some(p) = table[idx].as_mut() {
        p.brk = addr;
    }
}

/// Get current process page table root (PPN)
pub fn current_page_table_ppn() -> usize {
    let table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS.load(Ordering::Relaxed);
    table[idx].as_ref().map(|p| p.page_table_root).unwrap_or(0)
}

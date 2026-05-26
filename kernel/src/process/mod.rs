//! Process management: user-space processes with independent address spaces.

pub mod elf;

use core::sync::atomic::{AtomicUsize, Ordering};

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

/// Process control block
pub struct Process {
    pub pid: usize,
    /// Root page table (physical address of the Sv39 root page table)
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
}

impl Process {
    /// Create a new user process from an ELF binary embedded in the kernel.
    /// `elf_data` is the raw ELF file bytes (statically linked into the kernel).
    ///
    /// Phase 1: Maps user code/data/stack into the current kernel page table
    /// with U flag, so no satp switch is needed.
    pub fn from_elf(elf_data: &[u8]) -> Result<Self, &'static str> {
        // 1. Parse ELF
        let elf = elf::ElfFile::parse(elf_data)?;

        // 2. Get current kernel page table (Phase 1: shared page table)
        let kernel_pt = vmm::get_kernel_page_table();

        // 3. Load ELF segments — allocate physical frames, map with U flag, copy data
        let mut max_vaddr = 0usize;
        for segment in &elf.loadable_segments {
            let page_size = pmm::page_size();
            let start_page = segment.vaddr & !(page_size - 1);
            let end_page = (segment.vaddr + segment.mem_size + page_size - 1) & !(page_size - 1);

            for page_start in (start_page..end_page).step_by(page_size) {
                let frame = pmm::alloc_frame().ok_or("Out of memory for ELF segment")?;

                // Map in kernel page table with URWX flags so U-mode can execute
                vmm::map(kernel_pt, page_start, frame, vmm::PTEFlags::URWX);

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

        // 4. Map user stack in kernel page table (URW, no execute)
        // Map from USER_STACK_TOP downward for USER_STACK_PAGES pages
        for i in 0..USER_STACK_PAGES {
            let frame = pmm::alloc_frame().ok_or("Out of memory for user stack")?;
            let vaddr = USER_STACK_TOP - (i + 1) * pmm::page_size();
            vmm::map(kernel_pt, vaddr, frame, vmm::PTEFlags::URW);
        }

        // 5. Allocate kernel stack for this process (identity mapped already by vmm::init)
        let kstack_base = pmm::alloc_frame().ok_or("Out of memory for kernel stack")?;
        for _ in 0..3 {
            pmm::alloc_frame().ok_or("Out of memory for kernel stack")?;
        }
        let kernel_stack_top = kstack_base + 4 * pmm::page_size();

        // 5.5 Flush TLB so new user mappings take effect
        unsafe { core::arch::asm!("sfence.vma") };

        // 6. Set up initial brk
        let page_size = pmm::page_size();
        let initial_brk = (max_vaddr + page_size - 1) & !(page_size - 1);
        let brk = core::cmp::max(initial_brk, USER_HEAP_BASE);

        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);

        Ok(Self {
            pid,
            page_table_root: 0, // Phase 1: shared kernel page table, no separate PT
            kernel_stack_top,
            user_stack_top: USER_STACK_TOP,
            brk,
            initial_brk: brk,
            entry: elf.entry,
        })
    }
}

/// Get a mutable reference to the user page table from its PPN
pub fn get_user_page_table(ppn: usize) -> &'static mut vmm::PageTable {
    unsafe { &mut *((ppn << 12) as *mut vmm::PageTable) }
}

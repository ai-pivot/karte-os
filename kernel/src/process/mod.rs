//! Process management: user-space processes with independent address spaces.

pub mod elf;

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
pub const USER_MMAP_BASE: usize = 0x2000_0000;
pub const USER_MMAP_LIMIT: usize = 0x8000_0000; // 1.5GB mmap region
pub const USER_STACK_TOP: usize = 0x8000_0000; // 2GB — top of user stack
pub const USER_STACK_BASE: usize = 0x7FC0_0000; // 4MB stack
pub const USER_STACK_PAGES: usize = 64; // 256 KB actual stack (lazy-allocated on fault)
pub const KERNEL_STACK_PAGES: usize = 8; // 32 KB kernel stack

/// Process identifier allocator
pub(crate) static NEXT_PID: AtomicUsize = AtomicUsize::new(1);

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
    /// Parent process ID (0 for init process)
    pub ppid: usize,
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
    /// Exit code (valid when state == Exited)
    pub exit_code: usize,
    /// Index in PROCESS_TABLE of the child this process is waiting for.
    /// None if not waiting. Used by sys_exit to wake the parent.
    pub wait_child_idx: Option<usize>,
    /// Per-process file descriptor table
    pub fd_table: Option<crate::driver::fs::FdTable>,
    /// Trap context pointer (saved on kernel stack)
    pub trap_ctx_ptr: usize,
}

/// Copy kernel identity mappings into a user page table.
/// This is needed so that traps from U-mode can still access kernel code/data.
pub(crate) fn copy_kernel_mappings(user_pt: &mut vmm::PageTable) {
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
        // Map kernel code/data — start from 1MB to avoid conflicting with
        // user ELF segments which may use low addresses (e.g., shell.elf at 0x1000).
        // Map 1MB..512MB, skipping pages already mapped by ELF loader.
        vmm::identity_map_skip(user_pt, 0x10_0000, 0x2000_0000, vmm::PTEFlags::KRWX);
        // Map VGA text buffer at 0xB8000 (below 1MB, needed for console)
        vmm::map(user_pt, 0xB8000, 0xB8000, vmm::PTEFlags::KRW);
        // Map LAPIC/IOAPIC MMIO
        vmm::map(user_pt, 0xFEE0_0000, 0xFEE0_0000, vmm::PTEFlags::KRW);
        vmm::map(user_pt, 0xFEC0_0000, 0xFEC0_0000, vmm::PTEFlags::KRW);
        // Map PCI MMIO region
        vmm::identity_map(user_pt, 0xF000_0000, 0x1_0000_0000, vmm::PTEFlags::KRW);
    }
}

impl Process {
    /// Create a new user process from an ELF binary.
    ///
    /// User ELF segments are loaded at their original virtual addresses.
    /// The kernel identity-maps only its own code region (0..4MB), so user
    /// segments starting at 0x400000+ don't conflict.
    pub fn from_elf(elf_data: &[u8]) -> Result<Self, &'static str> {
        // 1. Parse ELF
        let elf = elf::ElfFile::parse(elf_data)?;

        // 2. Create independent user page table
        let user_pt = vmm::create_user_page_table();

        // 3. Load ELF segments into user page table FIRST
        // (before copy_kernel_mappings, so identity mappings don't interfere)

        // 4. Load ELF segments into user page table
        let mut max_vaddr = 0usize;
        for segment in &elf.loadable_segments {
            let page_size = pmm::page_size();
            let seg_vaddr_start = segment.vaddr;
            let seg_vaddr_end = segment.vaddr + segment.mem_size;
            let page_start = seg_vaddr_start & !(page_size - 1);
            let page_end = (seg_vaddr_end + page_size - 1) & !(page_size - 1);
            let is_executable = segment.flags & 1 != 0; // PF_X

            for vaddr in (page_start..page_end).step_by(page_size) {
                // Check if already mapped (multiple segments may share a page)
                let frame = match vmm::translate_user(user_pt, vaddr) {
                    Some(f) => f,
                    None => {
                        let f = pmm::alloc_frame().ok_or("Out of memory for ELF segment")?;
                        vmm::map_user(user_pt, vaddr, f, vmm::PTEFlags::URWX);
                        unsafe {
                            core::ptr::write_bytes(f as *mut u8, 0, page_size);
                        }
                        f
                    }
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

                    // Patch: replace `syscall` (0x0F 0x05) with `int 0x80` (0xCD 0x80)
                    // in executable segments. This makes Go binaries use our int 0x80
                    // syscall path instead of the SYSCALL instruction.
                    // The replacement is 2 bytes → 2 bytes, no size change.
                    #[cfg(target_arch = "x86_64")]
                    if is_executable {
                        let base = (frame + dst_offset) as *mut u8;
                        let patch_len = len;
                        unsafe {
                            let mut i = 0usize;
                            while i + 1 < patch_len {
                                let b0 = *base.add(i);
                                let b1 = *base.add(i + 1);
                                if b0 == 0x0F && b1 == 0x05 {
                                    // syscall → int 0x80
                                    *base.add(i) = 0xCD;
                                    *base.add(i + 1) = 0x80;
                                    i += 2;
                                } else {
                                    i += 1;
                                }
                            }
                        }
                    }
                }
            }
            if segment.vaddr + segment.mem_size > max_vaddr {
                max_vaddr = segment.vaddr + segment.mem_size;
            }
        }

        // 4. Copy kernel mappings AFTER loading ELF segments
        // (identity mappings must not interfere with ELF segment mapping)
        copy_kernel_mappings(user_pt);

        // 5. Map user stack in user page table (URW, no execute)
        for i in 0..USER_STACK_PAGES {
            let frame = pmm::alloc_frame().ok_or("Out of memory for user stack")?;
            let vaddr = USER_STACK_TOP - (i + 1) * pmm::page_size();
            vmm::map_user(user_pt, vaddr, frame, vmm::PTEFlags::URW);
        }

        // 6. Allocate kernel stack for this process
        let kstack_base = pmm::alloc_contiguous_frames(KERNEL_STACK_PAGES)
            .ok_or("Out of memory for kernel stack")?;
        let kernel_stack_top = kstack_base + KERNEL_STACK_PAGES * pmm::page_size();

        // 7. Store page_table_root as PPN
        let page_table_ppn = (user_pt as *const vmm::PageTable as usize) >> 12;

        // 8. Set up initial brk (after loaded segments, aligned to page)
        let page_size = pmm::page_size();
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
            user_stack_top: USER_STACK_TOP,
            brk,
            initial_brk: brk,
            entry: elf.entry,
            state: ProcessState::Ready,
            exit_code: 0,
            fd_table: Some(crate::driver::fs::FdTable::new()),
            wait_child_idx: None,
            trap_ctx_ptr: 0,
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
    pub fn from_elf_streaming<F>(read_fn: F) -> Result<Self, &'static str>
    where
        F: Fn(usize, &mut [u8]) -> Result<usize, ()>,
    {
        let page_size = pmm::page_size();

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

        // 4. Load ELF segments page-by-page
        let mut max_vaddr = 0usize;
        for seg_idx in 0..elf_info.num_segments {
            let seg = elf_info.segments[seg_idx].as_ref().unwrap();
            let seg_vaddr_start = seg.vaddr;
            let seg_vaddr_end = seg.vaddr + seg.mem_size;
            let page_start = seg_vaddr_start & !(page_size - 1);
            let page_end = (seg_vaddr_end + page_size - 1) & !(page_size - 1);
            let is_executable = seg.flags & 1 != 0; // PF_X

            for vaddr in (page_start..page_end).step_by(page_size) {
                // Check if already mapped (multiple segments may share a page)
                let frame = match vmm::translate_user(user_pt, vaddr) {
                    Some(f) => f,
                    None => {
                        let f = pmm::alloc_frame().ok_or("Out of memory for ELF segment")?;
                        vmm::map_user(user_pt, vaddr, f, vmm::PTEFlags::URWX);
                        // Zero-fill the entire frame
                        unsafe {
                            core::ptr::write_bytes(f as *mut u8, 0, page_size);
                        }
                        f
                    }
                };

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

                    // Patch: replace `syscall` (0x0F 0x05) with `int 0x80` (0xCD 0x80)
                    // in executable segments on x86_64.
                    #[cfg(target_arch = "x86_64")]
                    if is_executable {
                        let base = (frame + dst_offset) as *mut u8;
                        unsafe {
                            let mut i = 0usize;
                            while i + 1 < len {
                                let b0 = *base.add(i);
                                let b1 = *base.add(i + 1);
                                if b0 == 0x0F && b1 == 0x05 {
                                    *base.add(i) = 0xCD;
                                    *base.add(i + 1) = 0x80;
                                    i += 2;
                                } else {
                                    i += 1;
                                }
                            }
                        }
                    }
                }
            }
            if seg.vaddr + seg.mem_size > max_vaddr {
                max_vaddr = seg.vaddr + seg.mem_size;
            }
        }

        // 5. Copy kernel mappings AFTER loading ELF segments
        copy_kernel_mappings(user_pt);

        // 6. Map user stack in user page table (URW, no execute)
        for i in 0..USER_STACK_PAGES {
            let frame = pmm::alloc_frame().ok_or("Out of memory for user stack")?;
            let vaddr = USER_STACK_TOP - (i + 1) * page_size;
            vmm::map_user(user_pt, vaddr, frame, vmm::PTEFlags::URW);
        }

        // 7. Allocate kernel stack for this process
        let kstack_base = pmm::alloc_contiguous_frames(KERNEL_STACK_PAGES)
            .ok_or("Out of memory for kernel stack")?;
        let kernel_stack_top = kstack_base + KERNEL_STACK_PAGES * page_size;

        // 8. Store page_table_root as PPN
        let page_table_ppn = (user_pt as *const vmm::PageTable as usize) >> 12;

        // 9. Set up initial brk (after loaded segments, aligned to page)
        let initial_brk = (max_vaddr + page_size - 1) & !(page_size - 1);
        let brk = core::cmp::max(initial_brk, USER_HEAP_BASE);

        let entry = elf_info.entry;

        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);

        Ok(Self {
            pid,
            ppid: 0, // Set by caller
            page_table_root: page_table_ppn,
            kernel_stack_top,
            user_stack_top: USER_STACK_TOP,
            brk,
            initial_brk: brk,
            entry,
            state: ProcessState::Ready,
            exit_code: 0,
            fd_table: Some(crate::driver::fs::FdTable::new()),
            wait_child_idx: None,
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
        if p.page_table_root != 0 {
            crate::mm::vmm::free_user_page_table(p.page_table_root);
        }
        // Free kernel stack frames (KERNEL_STACK_PAGES * PAGE_SIZE)
        // Kernel stack is allocated from kernel_stack_top down
        let stack_bottom = p.kernel_stack_top - crate::process::KERNEL_STACK_PAGES * 4096;
        for offset in (0..crate::process::KERNEL_STACK_PAGES).step_by(4096) {
            crate::mm::pmm::dealloc_frame(stack_bottom + offset);
        }
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
/// Init (shell) has a fixed PID of 1 since it's not in the process table.
pub fn current_pid() -> usize {
    let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
    if idx >= crate::sched::MAX_TASKS {
        return 1; // init process
    }
    let table = PROCESS_TABLE.lock();
    table[idx].as_ref().map(|p| p.pid).unwrap_or(0)
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
    let mut table = PROCESS_TABLE.lock();
    let idx = CURRENT_PROCESS[hartid()].load(Ordering::Relaxed);
    let proc = table[idx].as_mut().expect("No current process");
    let fd_table = proc.fd_table.as_mut().expect("No FD table");
    f(fd_table)
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

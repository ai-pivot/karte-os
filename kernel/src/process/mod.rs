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
pub const USER_MMAP_BASE: usize = 0x2000_0000;
pub const USER_MMAP_LIMIT: usize = 0x8000_0000_0000; // 8TB mmap region for Go heap arenas
pub const USER_STACK_TOP: usize = 0x8000_0000; // 2GB — top of user stack
pub const USER_STACK_BASE: usize = 0x7F00_0000; // 16MB stack region (address space)
pub const USER_STACK_PAGES: usize = 512; // 2 MB pre-mapped stack (Go g0 needs ~1MB+)
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
    /// Per-process file descriptor table
    pub fd_table: Option<crate::driver::fs::FdTable>,
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
        // Identity-map from 1MB to end of physical RAM, but SKIP any pages
        // already mapped by the ELF loader. This preserves user program
        // virtual addresses (e.g., xbot at 0x400000+) while still providing
        // access to kernel code, heap, and data structures.
        let ram_end = crate::mm::pmm::total_memory();
        vmm::identity_map_skip(user_pt, 0x10_0000, ram_end, vmm::PTEFlags::KRWX);

        // Map VGA text buffer at 0xB8000 (2 pages)
        vmm::map(user_pt, 0xB8000, 0xB8000, vmm::PTEFlags::KRW);
        vmm::map(user_pt, 0xB9000, 0xB9000, vmm::PTEFlags::KRW);
        // Map LAPIC/IOAPIC MMIO
        vmm::map(user_pt, 0xFEE0_0000, 0xFEE0_0000, vmm::PTEFlags::KRW);
        vmm::map(user_pt, 0xFEC0_0000, 0xFEC0_0000, vmm::PTEFlags::KRW);
        // Map PCI MMIO region
        vmm::identity_map_2mb(user_pt, 0xF000_0000, 0x1_0000_0000, vmm::PTEFlags::KRW);

        // Map kernel stack pages into user page table.
        let kstack_base = kernel_stack_top - KERNEL_STACK_PAGES * 4096;
        for addr in (kstack_base..kernel_stack_top).step_by(4096) {
            vmm::map(user_pt, addr, addr, vmm::PTEFlags::KRW);
        }
    }
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
        let kstack_base = pmm::alloc_contiguous_frames(KERNEL_STACK_PAGES)
            .ok_or("Out of memory for kernel stack")?;
        let kernel_stack_top = kstack_base + KERNEL_STACK_PAGES * pmm::page_size();

        // 4. Load ELF segments into user page table FIRST
        // (before copy_kernel_mappings, so identity mappings don't interfere)
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

                    // NOTE: We used to patch `syscall` (0x0F 0x05) to `int 0x80` (0xCD 0x80)
                    // here. This is NO LONGER needed since we now have proper MSR-based
                    // SYSCALL/SYSRET support. The binary's native `syscall` instruction
                    // works correctly via the LSTAR entry point → dispatch_linux_raw().
                    let _ = is_executable;
                }
            }
            if segment.vaddr + segment.mem_size > max_vaddr {
                max_vaddr = segment.vaddr + segment.mem_size;
            }
        }

        // 4. Copy kernel mappings AFTER loading ELF segments
        // (identity mappings must not interfere with ELF segment mapping)
        copy_kernel_mappings(user_pt, kernel_stack_top);

        // 5. Map user stack in user page table (URW, no execute)
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
            fd_table: Some(crate::driver::fs::FdTable::new()),
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
    ) -> Result<Self, &'static str>
    where
        F: Fn(usize, &mut [u8]) -> Result<usize, ()>,
    {
        let page_size = pmm::page_size();

        // 0. Allocate kernel stack (needed before copy_kernel_mappings)
        let kstack_base = pmm::alloc_contiguous_frames(KERNEL_STACK_PAGES)
            .ok_or("Out of memory for kernel stack")?;
        let kernel_stack_top = kstack_base + KERNEL_STACK_PAGES * page_size;

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

        // 4. Load ELF segments page-by-page
        let mut max_vaddr = 0usize;
        for seg_idx in 0..elf_info.num_segments {
            let seg = elf_info.segments[seg_idx].as_ref().unwrap();
            let seg_vaddr_start = seg.vaddr;
            let seg_vaddr_end = seg.vaddr + seg.mem_size;
            let page_start = seg_vaddr_start & !(page_size - 1);
            let page_end = (seg_vaddr_end + page_size - 1) & !(page_size - 1);
            let is_executable = seg.flags & 1 != 0; // PF_X
            let num_pages = (page_end - page_start) / page_size;

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

                    // NOTE: No longer patching syscall→int 0x80. MSR SYSCALL/SYSRET
                    // is properly configured. The binary's native `syscall` instruction
                    // works via LSTAR → dispatch_linux_raw().
                    let _ = is_executable;
                }
            }
            if seg.vaddr + seg.mem_size > max_vaddr {
                max_vaddr = seg.vaddr + seg.mem_size;
            }
        }

        // 5. Copy kernel mappings AFTER loading ELF segments
        copy_kernel_mappings(user_pt, kernel_stack_top);

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

        // ── Phase 1: Write string data near the top of the stack ──
        // We write strings from stack_top - 16 (after 16B random) downward.
        // Track each string's address for the pointer arrays.

        // Write random bytes first (16 bytes at stack_top - 16)
        let random_addr = stack_top - 16;
        unsafe {
            write_u64_to_user(user_pt, random_addr, 0x12345678_9ABCDEF0u64);
            write_u64_to_user(user_pt, random_addr + 8, 0xDEADBEEF_FEEDFACEu64);
        }

        // Build envp strings: "KEY=VALUE\0"
        // We'll first compute the total string area size, then write from top down.
        let envp_strs: alloc::vec::Vec<alloc::vec::Vec<u8>> = envp
            .iter()
            .map(|(k, v)| {
                let mut s = alloc::vec::Vec::new();
                s.extend_from_slice(k);
                s.push(b'=');
                s.extend_from_slice(v);
                s.push(0); // null terminator
                s
            })
            .collect();

        // argv strings: each null-terminated
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

        // Calculate total string area size
        let strings_size: usize = argv_strs.iter().map(|s| s.len()).sum::<usize>()
            + envp_strs.iter().map(|s| s.len()).sum::<usize>();

        // String area starts at: random_addr - strings_size
        let strings_start = random_addr - strings_size;

        // Write strings and record their addresses
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

        // ── Phase 2: Calculate and write the metadata area below strings ──
        let auxv_data: [(usize, usize); 12] = [
            (AT_SYSINFO_EHDR, 0), // No VDSO
            (AT_PHDR, phdr_addr),
            (AT_PHENT, elf_info.phent),
            (AT_PHNUM, elf_info.phnum as usize),
            (AT_PAGESZ, page_size),
            (AT_ENTRY, elf_info.entry),
            (AT_UID, 0),
            (AT_EUID, 0),
            (AT_GID, 0),
            (AT_EGID, 0),
            (AT_RANDOM, random_addr), // point to random bytes
            (AT_NULL, 0),
        ];

        let argc = argv_ptrs.len();
        // Layout from low to high:
        //   argc (8)
        //   argv[0..argc] (argc * 8)
        //   argv NULL (8)
        //   envp[0..n] (n * 8)
        //   envp NULL (8)
        //   auxv (12 * 16 = 192)
        let metadata_size = 8 + (argc + 1) * 8 + (envp_ptrs.len() + 1) * 8 + auxv_data.len() * 16;

        let metadata_end = strings_start;
        let metadata_start = metadata_end - metadata_size;
        // Align to 16 bytes
        let metadata_start = metadata_start & !0xF;
        let initial_rsp = metadata_start;

        // Write metadata from initial_rsp upward
        let mut pos = initial_rsp;

        // argc
        unsafe {
            write_u64_to_user(user_pt, pos, argc as u64);
        }
        pos += 8;

        // argv pointers
        for &ptr in &argv_ptrs {
            unsafe {
                write_u64_to_user(user_pt, pos, ptr as u64);
            }
            pos += 8;
        }
        // argv NULL terminator
        unsafe {
            write_u64_to_user(user_pt, pos, 0);
        }
        pos += 8;

        // envp pointers
        for &ptr in &envp_ptrs {
            unsafe {
                write_u64_to_user(user_pt, pos, ptr as u64);
            }
            pos += 8;
        }
        // envp NULL terminator
        unsafe {
            write_u64_to_user(user_pt, pos, 0);
        }
        pos += 8;

        // auxv entries
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
            fd_table: Some(crate::driver::fs::FdTable::new()),
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

/// Set exit code for a process by its table index (used by kill_clone_children).
pub fn set_exit_code_by_index(idx: usize, code: usize) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table[idx].as_mut() {
        p.exit_code = code;
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
    for idx in indices_to_kill {
        // CLONE_CHILD_CLEARTID: notify futex waiters
        {
            let table = PROCESS_TABLE.lock();
            if let Some(p) = table[idx].as_ref() {
                if p.child_tid_ptr != 0 {
                    let tid_ptr = p.child_tid_ptr;
                    drop(table);
                    unsafe {
                        core::ptr::write_volatile(tid_ptr as *mut i32, 0);
                    }
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
    }
    // On SMP, other cores may still be running clone child threads.
    // Send IPI to force immediate reschedule on all other cores.
    #[cfg(target_arch = "x86_64")]
    crate::arch::lapic::broadcast_reschedule();
}

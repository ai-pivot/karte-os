// kernel/src/mm/pmm.rs — Physical Memory Manager (bitmap frame allocator)

use spin::Mutex;

const PAGE_SIZE: usize = 4096;
#[cfg(target_arch = "riscv64")]
const MEMORY_START: usize = 0x8020_0000;

#[cfg(target_arch = "x86_64")]
const MEMORY_START: usize = 0x0020_0000; // 2MB — typical x86_64 kernel load address

#[cfg(target_arch = "riscv64")]
const MEMORY_SIZE: usize = 2048 * 1024 * 1024;

// On x86_64, MEMORY_SIZE is set dynamically from multiboot2 info.
// See `init_with_size()` below. This constant is only a fallback.
#[cfg(target_arch = "x86_64")]
static mut MEMORY_SIZE: usize = 128 * 1024 * 1024; // 128MB default, updated by multiboot2

unsafe extern "C" {
    static _ekernel: u8;
}

static FRAME_ALLOCATOR: Mutex<Option<FrameAllocator>> = Mutex::new(None);

// ─── Pre-zeroed page pool ────────────────────────────────────────────────
/// Pool of pre-zeroed physical frames for fast page fault handling.
/// When the PF handler needs a zeroed frame, it pops from this pool
/// instead of allocating + zeroing (saves ~4KB write per PF).
const ZEROED_POOL_SIZE: usize = 32;
static ZEROED_POOL: Mutex<ZeroedPool> = Mutex::new(ZeroedPool::new());

struct ZeroedPool {
    frames: [usize; ZEROED_POOL_SIZE],
    count: usize,
}

impl ZeroedPool {
    const fn new() -> Self {
        Self {
            frames: [0; ZEROED_POOL_SIZE],
            count: 0,
        }
    }

    fn pop(&mut self) -> Option<usize> {
        if self.count > 0 {
            self.count -= 1;
            Some(self.frames[self.count])
        } else {
            None
        }
    }

    fn push(&mut self, frame: usize) {
        if self.count < ZEROED_POOL_SIZE {
            self.frames[self.count] = frame;
            self.count += 1;
        } else {
            // Pool full, just deallocate
            dealloc_frame(frame);
        }
    }
}

/// Allocate a pre-zeroed frame. Falls back to regular alloc + zero if pool empty.
pub fn alloc_zeroed_frame() -> Option<usize> {
    // Try the zeroed pool first
    if let Some(frame) = ZEROED_POOL.lock().pop() {
        return Some(frame);
    }
    // Fall back: allocate and zero manually
    let frame = alloc_frame()?;
    zero_frame(frame);
    Some(frame)
}

/// Zero a physical frame using its high-half virtual mapping.
fn zero_frame(frame: usize) {
    let vaddr = crate::mm::vmm::phys_to_virt(frame);
    unsafe {
        core::ptr::write_bytes(vaddr as *mut u8, 0, PAGE_SIZE);
    }
}

/// Refill the zeroed page pool. Called from idle loop to pre-zero frames
/// during otherwise-wasted CPU cycles.
#[cfg(target_arch = "x86_64")]
pub fn refill_zeroed_pool() {
    let mut pool = ZEROED_POOL.lock();
    while pool.count < ZEROED_POOL_SIZE {
        drop(pool); // Release lock during alloc+zero
        if let Some(frame) = alloc_frame() {
            zero_frame(frame);
            pool = ZEROED_POOL.lock();
            pool.push(frame);
        } else {
            break; // Out of memory
        }
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn refill_zeroed_pool() {}

struct FrameAllocator {
    start: usize,
    // Bitmap: each bit represents one 4KB frame
    bitmap: &'static mut [u64],
    total_frames: usize,
    next_free: usize,
}

impl FrameAllocator {
    fn new() -> Self {
        let kernel_end = unsafe { &_ekernel as *const u8 as usize };
        #[cfg(target_arch = "x86_64")]
        let kernel_end = crate::platform::x86_64::virt_to_phys(kernel_end);
        let start = (kernel_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1); // Align up
        #[cfg(target_arch = "riscv64")]
        let mem_size = MEMORY_SIZE;
        #[cfg(target_arch = "x86_64")]
        let mem_size = unsafe { MEMORY_SIZE };
        let end = MEMORY_START + mem_size;
        let total_frames = (end - start) / PAGE_SIZE;

        // Calculate bitmap size (in 64-bit words)
        let bitmap_words = (total_frames + 63) / 64;
        let bitmap_size = bitmap_words * 8;

        // Place bitmap right after kernel, before managed frames
        let bitmap_start = start;
        let managed_start = (bitmap_start + bitmap_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let managed_frames = (end - managed_start) / PAGE_SIZE;

        // On x86_64 high-half kernel, access the bitmap via the direct map
        // (phys_to_virt).  The bitmap is at a physical address that may be
        // above the identity-mapped range, so we MUST use phys_to_virt.
        #[cfg(target_arch = "x86_64")]
        let bitmap_vaddr = crate::mm::vmm::phys_to_virt(bitmap_start);
        #[cfg(not(target_arch = "x86_64"))]
        let bitmap_vaddr = bitmap_start;

        let bitmap =
            unsafe { core::slice::from_raw_parts_mut(bitmap_vaddr as *mut u64, bitmap_words) };

        // Clear the bitmap — all frames start as free
        for word in bitmap.iter_mut() {
            *word = 0;
        }

        let allocator = Self {
            start: managed_start,
            bitmap,
            total_frames: managed_frames,
            next_free: 0,
        };
        allocator.debug_init_info(kernel_end, start, managed_start, end);
        allocator
    }

    #[cfg(debug_assertions)]
    fn debug_init_info(&self, kernel_end: usize, start: usize, managed_start: usize, end: usize) {
        crate::console_println!(
            "[pmm] kernel_end={:#x} start={:#x} managed_start={:#x} end={:#x}",
            kernel_end,
            start,
            managed_start,
            end
        );
        crate::console_println!(
            "[pmm] total_frames={} managed_frames={} bitmap_words={} bitmap_frames={}",
            (end - start) / PAGE_SIZE,
            self.total_frames,
            ((end - start) / PAGE_SIZE + 63) / 64,
            (managed_start - start + PAGE_SIZE - 1) / PAGE_SIZE
        );
    }
    #[cfg(not(debug_assertions))]
    fn debug_init_info(&self, _a: usize, _b: usize, _c: usize, _d: usize) {}

    fn alloc(&mut self) -> Option<usize> {
        // Search from next_free
        for i in self.next_free..self.total_frames {
            let word = i / 64;
            let bit = i % 64;
            if self.bitmap[word] & (1u64 << bit) == 0 {
                self.bitmap[word] |= 1u64 << bit;
                self.next_free = i + 1;
                let addr = self.start + i * PAGE_SIZE;
                return Some(addr);
            }
        }
        // Wrap around
        for i in 0..self.next_free {
            let word = i / 64;
            let bit = i % 64;
            if self.bitmap[word] & (1u64 << bit) == 0 {
                self.bitmap[word] |= 1u64 << bit;
                self.next_free = i + 1;
                let addr = self.start + i * PAGE_SIZE;
                return Some(addr);
            }
        }
        None
    }

    fn alloc_contiguous(&mut self, count: usize) -> Option<usize> {
        // Scan bitmap for `count` consecutive free frames.
        // This is O(total_frames) in the worst case, which is acceptable
        // for a 128MB memory pool (~32K frames).
        if count == 0 {
            return None;
        }
        let mut run_start = 0;
        let mut run_length = 0usize;
        for i in 0..self.total_frames {
            let word = i / 64;
            let bit = i % 64;
            if self.bitmap[word] & (1u64 << bit) == 0 {
                if run_length == 0 {
                    run_start = i;
                }
                run_length += 1;
                if run_length == count {
                    // Mark all frames in the run as allocated
                    for j in run_start..run_start + count {
                        let w = j / 64;
                        let b = j % 64;
                        self.bitmap[w] |= 1u64 << b;
                    }
                    self.next_free = run_start + count;
                    let addr = self.start + run_start * PAGE_SIZE;
                    return Some(addr);
                }
            } else {
                run_length = 0;
            }
        }
        None
    }

    fn dealloc(&mut self, frame: usize) {
        let idx = (frame - self.start) / PAGE_SIZE;
        if idx < self.total_frames {
            let word = idx / 64;
            let bit = idx % 64;
            self.bitmap[word] &= !(1u64 << bit);
            if idx < self.next_free {
                self.next_free = idx;
            }
        }
    }
}

pub fn init() {
    let allocator = FrameAllocator::new();
    let available_mb = allocator.total_frames * PAGE_SIZE / 1024 / 1024;
    *FRAME_ALLOCATOR.lock() = Some(allocator);
    crate::console_println!("[pmm] Initialized: {} MB available", available_mb);
}

/// Initialize PMM with a specific memory size (x86_64 only).
/// Called from kmain after parsing multiboot2 info.
#[cfg(target_arch = "x86_64")]
pub fn init_with_size(mem_size: usize) {
    unsafe {
        MEMORY_SIZE = mem_size;
    }
    let mut allocator = FrameAllocator::new();
    let available_mb = allocator.total_frames * PAGE_SIZE / 1024 / 1024;
    *FRAME_ALLOCATOR.lock() = Some(allocator);
    crate::console_println!(
        "[pmm] Initialized: {} MB available (total RAM: {} MB)",
        available_mb,
        mem_size / 1024 / 1024
    );
    // On UEFI boots, mark non-conventional memory regions as used so PMM
    // never hands out frames that overlap firmware/MMIO/reserved areas.
    // Without this, real hardware may allocate a frame whose physical
    // address points to ACPI/firmware memory → silent corruption when the
    // ELF loader or page-table allocator writes to it. QEMU's UEFI has a
    // simple map (0..512MB all conventional) so the bug is invisible there.
    mark_efi_reserved();
}

/// EFI memory descriptor (UEFI spec §7.2 "Memory Descriptor").
#[repr(C)]
#[derive(Clone, Copy)]
struct EfiMemoryDescriptor {
    entry_type: u32,
    _pad: u32,
    phys_start: u64,
    virt_start: u64,
    num_pages: u64,
    attribute: u64,
}

const EFI_CONVENTIONAL_MEMORY: u32 = 7;
const EFI_BOOT_SERVICES_CODE: u32 = 5;
const EFI_BOOT_SERVICES_DATA: u32 = 6;
const EFI_LOADER_CODE: u32 = 2;
const EFI_LOADER_DATA: u32 = 3;

/// Walk the UEFI memory map (if present at 0x20000) and mark every region
/// that is NOT conventional/boot-services/loader memory as allocated in the
/// PMM bitmap. This prevents the frame allocator from returning frames that
/// overlap ACPI, MMIO, runtime, or reserved firmware memory on real hardware.
#[cfg(target_arch = "x86_64")]
fn mark_efi_reserved() {
    // BootInfo at 0x10000, layout (u32 offsets):
    //   [0]=magic  [8]=memmap_present  [9..10]=memmap_size
    //   [11]=desc_size  [12]=desc_ver
    let bi = 0x10000usize as *const u32;
    let magic = unsafe { core::ptr::read_volatile(bi) };
    if magic != 0x474F5046 {
        return; // Not EFI stub booted
    }
    let memmap_present = unsafe { core::ptr::read_volatile(bi.add(8)) };
    if memmap_present == 0 {
        return; // No memory map captured
    }
    let memmap_size = unsafe {
        let lo = core::ptr::read_volatile(bi.add(9) as *const u32) as u64;
        let hi = core::ptr::read_volatile(bi.add(10) as *const u32) as u64;
        (hi << 32) | lo
    } as usize;
    let desc_size = unsafe { core::ptr::read_volatile(bi.add(11)) } as usize;

    if desc_size == 0 || memmap_size == 0 {
        return;
    }

    let desc_count = memmap_size / desc_size;
    let map_base = 0x20000usize;

    let mut marked = 0usize;
    let mut conventional = 0usize;

    crate::console_println!(
        "[pmm] EFI memmap: {} descriptors, desc_size={}",
        desc_count,
        desc_size
    );

    for i in 0..desc_count {
        let desc_ptr = (map_base + i * desc_size) as *const EfiMemoryDescriptor;
        let desc = unsafe { core::ptr::read_volatile(desc_ptr) };
        let is_usable = desc.entry_type == EFI_CONVENTIONAL_MEMORY
            || desc.entry_type == EFI_BOOT_SERVICES_CODE
            || desc.entry_type == EFI_BOOT_SERVICES_DATA
            || desc.entry_type == EFI_LOADER_CODE
            || desc.entry_type == EFI_LOADER_DATA;

        // Dump first 16 + last 4 descriptors to see the full memory layout
        // without flooding the console on firmware with 500+ entries.
        if i < 16 || i >= desc_count.saturating_sub(4) {
            let type_name = match desc.entry_type {
                0 => "Reserved",
                1 => "LoaderCode",
                2 => "LoaderData",
                3 => "BS_Code",
                4 => "BS_Data",
                5 => "RT_Code",
                6 => "RT_Data",
                7 => "Conv",
                8 => "Unusable",
                9 => "ACPI_Recl",
                10 => "ACPI_NVS",
                11 => "MMIO",
                12 => "MMIO_Port",
                13 => "PalCode",
                14 => "Persistent",
                _ => "Unknown",
            };
            let kb = (desc.num_pages as usize) * PAGE_SIZE / 1024;
            crate::console_println!(
                "  [{:>3}] {:>10} {:#012x}-{:#012x} ({} KB){}",
                i,
                type_name,
                desc.phys_start,
                desc.phys_start + desc.num_pages * 4096,
                kb,
                if is_usable { "" } else { " <- reserved" }
            );
        } else if i == 16 {
            crate::console_println!("  ... ({} more entries) ...", desc_count - 20);
        }

        if is_usable {
            conventional += 1;
            continue;
        }
        // Mark every 4KB frame in this region as used in the PMM bitmap.
        let region_start = desc.phys_start as usize;
        let region_end = region_start + (desc.num_pages as usize) * PAGE_SIZE;
        marked += mark_range_used(region_start, region_end);
    }

    crate::console_println!(
        "[pmm] EFI memmap: {} conventional, marked {} reserved frames ({} KB)",
        conventional,
        marked,
        marked * PAGE_SIZE / 1024
    );
}

/// Mark all frames in [phys_start, phys_end) as used in the PMM bitmap.
/// Returns the number of frames marked. Frames outside the managed range
/// are silently skipped (they were never allocatable anyway).
#[cfg(target_arch = "x86_64")]
fn mark_range_used(phys_start: usize, phys_end: usize) -> usize {
    let mut alloc = FRAME_ALLOCATOR.lock();
    let allocator = match alloc.as_mut() {
        Some(a) => a,
        None => return 0,
    };
    let page_start = (phys_start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let page_end = phys_end & !(PAGE_SIZE - 1);
    if page_end <= page_start {
        return 0;
    }
    let mut count = 0;
    let mut pa = page_start;
    while pa < page_end {
        let idx: i64 = (pa as i64 - allocator.start as i64) / PAGE_SIZE as i64;
        if idx >= 0 && (idx as usize) < allocator.total_frames {
            let i = idx as usize;
            let word = i / 64;
            let bit = i % 64;
            if allocator.bitmap[word] & (1u64 << bit) == 0 {
                allocator.bitmap[word] |= 1u64 << bit;
                count += 1;
            }
        }
        pa += PAGE_SIZE;
    }
    count
}

/// Track frame allocation to detect double-mapping bugs.
/// Returns (alloc_count, used_count, next_free).
pub fn stats() -> (usize, usize, usize) {
    let alloc = FRAME_ALLOCATOR.lock();
    match alloc.as_ref() {
        Some(alloc) => {
            let mut used = 0;
            for i in 0..alloc.total_frames {
                let word = i / 64;
                let bit = i % 64;
                if alloc.bitmap[word] & (1u64 << bit) != 0 {
                    used += 1;
                }
            }
            (alloc.total_frames, used, alloc.next_free)
        }
        None => (0, 0, 0),
    }
}

/// Return the total physical memory size (end address).
/// Used by copy_kernel_mappings to identity-map the full kernel address space.
#[cfg(target_arch = "x86_64")]
pub fn total_memory() -> usize {
    MEMORY_START + unsafe { MEMORY_SIZE }
}

pub fn alloc_frame() -> Option<usize> {
    FRAME_ALLOCATOR.lock().as_mut()?.alloc()
}

pub fn dealloc_frame(frame: usize) {
    if let Some(ref mut alloc) = *FRAME_ALLOCATOR.lock() {
        alloc.dealloc(frame);
    }
}

pub fn alloc_contiguous_frames(count: usize) -> Option<usize> {
    FRAME_ALLOCATOR.lock().as_mut()?.alloc_contiguous(count)
}

pub const fn page_size() -> usize {
    PAGE_SIZE
}

/// Debug: return (total_frames, used_frames, next_free)
pub fn debug_stats() -> (usize, usize, usize) {
    let guard = FRAME_ALLOCATOR.lock();
    match guard.as_ref() {
        Some(alloc) => {
            let mut used = 0usize;
            for i in 0..alloc.total_frames {
                let word = i / 64;
                let bit = i % 64;
                if alloc.bitmap[word] & (1u64 << bit) != 0 {
                    used += 1;
                }
            }
            (alloc.total_frames, used, alloc.next_free)
        }
        None => (0, 0, 0),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── PMM Tests ──");

    // Test 1: Basic allocation
    crate::test::run_test("pmm_alloc_returns_valid_frame", || {
        match alloc_frame() {
            Some(frame) => {
                // Frame should be page-aligned
                frame % page_size() == 0
            }
            None => false,
        }
    });

    // Test 2: Allocate and deallocate
    crate::test::run_test("pmm_alloc_dealloc_cycle", || {
        let frame = alloc_frame();
        if frame.is_none() {
            return false;
        }
        let f = frame.unwrap();

        // Allocate a second frame (should be different)
        let frame2 = alloc_frame();
        if frame2.is_none() {
            return false;
        }
        let f2 = frame2.unwrap();

        let different = f != f2;

        // Free both
        dealloc_frame(f);
        dealloc_frame(f2);

        // Re-allocate should succeed
        let f3 = alloc_frame();
        f3.is_some() && different
    });

    // Test 3: Multiple allocations are unique
    crate::test::run_test("pmm_multiple_allocs_unique", || {
        let mut frames = [0usize; 16];
        let mut all_unique = true;

        for i in 0..16 {
            match alloc_frame() {
                Some(f) => {
                    // Check no duplicates
                    for j in 0..i {
                        if frames[j] == f {
                            all_unique = false;
                        }
                    }
                    frames[i] = f;
                }
                None => return false,
            }
        }

        // Clean up
        for f in frames {
            dealloc_frame(f);
        }

        all_unique
    });

    // Test 4: Dealloc then realloc returns a freed frame
    crate::test::run_test("pmm_dealloc_reuse", || {
        let f1 = alloc_frame();
        if f1.is_none() {
            return false;
        }
        let addr1 = f1.unwrap();

        dealloc_frame(addr1);

        // Allocate again — should eventually get the same frame back
        // (not guaranteed immediately, but should succeed)
        let f2 = alloc_frame();
        if f2.is_none() {
            return false;
        }
        dealloc_frame(f2.unwrap());

        true
    });

    // Test 5: Page size is 4096
    crate::test::run_test("pmm_page_size_is_4096", || page_size() == 4096);

    // Test 6: Allocated frames are in valid range
    crate::test::run_test("pmm_frames_in_valid_range", || {
        let frames: [Option<usize>; 4] =
            [alloc_frame(), alloc_frame(), alloc_frame(), alloc_frame()];

        #[cfg(target_arch = "riscv64")]
        let mem_end = MEMORY_START + MEMORY_SIZE;
        #[cfg(target_arch = "x86_64")]
        let mem_end = MEMORY_START + unsafe { MEMORY_SIZE };

        let mut valid = true;
        for f in &frames {
            match f {
                Some(addr) => {
                    // Should be within managed physical memory range
                    if *addr < MEMORY_START || *addr >= mem_end {
                        valid = false;
                    }
                }
                None => valid = false,
            }
        }

        for f in frames.iter().flatten() {
            dealloc_frame(*f);
        }

        valid
    });
}

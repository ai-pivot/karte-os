// kernel/src/mm/pmm.rs — Physical Memory Manager (bitmap frame allocator)

use spin::Mutex;

const PAGE_SIZE: usize = 4096;

#[cfg(target_arch = "riscv64")]
const MEMORY_START: usize = 0x8020_0000;

#[cfg(target_arch = "x86_64")]
const MEMORY_START: usize = 0x0020_0000; // 2MB — typical x86_64 kernel load address

#[cfg(target_arch = "riscv64")]
const MEMORY_SIZE: usize = 128 * 1024 * 1024; // 128MB

#[cfg(target_arch = "x86_64")]
const MEMORY_SIZE: usize = 128 * 1024 * 1024; // 128MB

unsafe extern "C" {
    static _ekernel: u8;
}

static FRAME_ALLOCATOR: Mutex<Option<FrameAllocator>> = Mutex::new(None);

struct FrameAllocator {
    start: usize,
    #[allow(dead_code)]
    end: usize,
    // Bitmap: each bit represents one 4KB frame
    bitmap: &'static mut [u64],
    total_frames: usize,
    next_free: usize,
}

impl FrameAllocator {
    fn new() -> Self {
        let kernel_end = unsafe { &_ekernel as *const u8 as usize };
        let start = (kernel_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1); // Align up
        let end = MEMORY_START + MEMORY_SIZE;
        let total_frames = (end - start) / PAGE_SIZE;

        // Calculate bitmap size (in 64-bit words)
        let bitmap_words = (total_frames + 63) / 64;
        let bitmap_size = bitmap_words * 8;

        // Place bitmap right after kernel, before managed frames
        let bitmap_start = start;
        let managed_start = (bitmap_start + bitmap_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let managed_frames = (end - managed_start) / PAGE_SIZE;

        let bitmap =
            unsafe { core::slice::from_raw_parts_mut(bitmap_start as *mut u64, bitmap_words) };

        // Mark bitmap region as used
        let bitmap_frames = (managed_start - start + PAGE_SIZE - 1) / PAGE_SIZE;
        for i in 0..bitmap_frames {
            let word = i / 64;
            let bit = i % 64;
            if word < bitmap.len() {
                bitmap[word] |= 1u64 << bit;
            }
        }

        Self {
            start: managed_start,
            end,
            bitmap,
            total_frames: managed_frames,
            next_free: bitmap_frames,
        }
    }

    fn alloc(&mut self) -> Option<usize> {
        // Search from next_free
        for i in self.next_free..self.total_frames {
            let word = i / 64;
            let bit = i % 64;
            if self.bitmap[word] & (1u64 << bit) == 0 {
                self.bitmap[word] |= 1u64 << bit;
                self.next_free = i + 1;
                return Some(self.start + i * PAGE_SIZE);
            }
        }
        // Wrap around
        for i in 0..self.next_free {
            let word = i / 64;
            let bit = i % 64;
            if self.bitmap[word] & (1u64 << bit) == 0 {
                self.bitmap[word] |= 1u64 << bit;
                self.next_free = i + 1;
                return Some(self.start + i * PAGE_SIZE);
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
                    return Some(self.start + run_start * PAGE_SIZE);
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

        let mut valid = true;
        for f in &frames {
            match f {
                Some(addr) => {
                    // Should be above kernel end and below 128MB
                    if *addr < 0x8020_0000 || *addr >= 0x8020_0000 + 128 * 1024 * 1024 {
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

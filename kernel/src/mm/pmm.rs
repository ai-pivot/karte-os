// kernel/src/mm/pmm.rs — Physical Memory Manager (bitmap frame allocator)

use spin::Mutex;

const PAGE_SIZE: usize = 4096;
const MEMORY_START: usize = 0x8020_0000;
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

        let bitmap = unsafe {
            core::slice::from_raw_parts_mut(bitmap_start as *mut u64, bitmap_words)
        };

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
    crate::console_println!(
        "[pmm] Initialized: {} MB available",
        available_mb
    );
}

pub fn alloc_frame() -> Option<usize> {
    FRAME_ALLOCATOR.lock().as_mut()?.alloc()
}

pub fn dealloc_frame(frame: usize) {
    if let Some(ref mut alloc) = *FRAME_ALLOCATOR.lock() {
        alloc.dealloc(frame);
    }
}

pub const fn page_size() -> usize {
    PAGE_SIZE
}

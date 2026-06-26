//! Limine Boot Protocol integration for KarteOS.
//! Uses Limine native protocol to get GOP framebuffer on UEFI systems.
//! Falls back to multiboot2 on non-Limine boots (QEMU).
//!
//! Reference: https://github.com/limine-bootloader/limine/blob/trunk/limine.h

use core::sync::atomic::{AtomicBool, Ordering};

// ─── Limine request/response structures ────────────────────────────────

/// Base Limine request identifier
const LIMINE_COMMON_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];

/// Framebuffer request magic
const LIMINE_FRAMEBUFFER_MAGIC: [u64; 2] = [0x9d5827dcd881dd75, 0xa3148604f6fab11b];

/// Memory map request magic
const LIMINE_MEMMAP_MAGIC: [u64; 2] = [0x67cf3d9d378a806f, 0xe304acdfc50c3c62];

#[repr(C)]
pub struct LimineFramebuffer {
    pub address: *mut u8,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
    _unused: [u8; 7],
    pub edid_size: u64,
    pub edid: *mut u8,
    _revision: u64, // revision 1+
}

#[repr(C)]
pub struct LimineFramebufferResponse {
    pub revision: u64,
    pub framebuffer_count: u64,
    pub framebuffers: *mut *mut LimineFramebuffer,
}

#[repr(C)]
pub struct LimineFramebufferRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut LimineFramebufferResponse,
}

#[repr(C)]
pub struct LimineMemmapEntry {
    pub base: u64,
    pub length: u64,
    pub entry_type: u64, // 0 = USABLE, 1 = RESERVED, etc.
}

#[repr(C)]
pub struct LimineMemmapResponse {
    pub revision: u64,
    pub entry_count: u64,
    pub entries: *mut *mut LimineMemmapEntry,
}

#[repr(C)]
pub struct LimineMemmapRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut LimineMemmapResponse,
}

// ─── Limine requests (placed in .limine_reqs section) ──────────────────

/// Framebuffer request — Limine fills response pointer at boot
#[used]
#[unsafe(link_section = ".limine_reqs")]
pub static FRAMEBUFFER_REQUEST: LimineFramebufferRequest = LimineFramebufferRequest {
    id: [
        LIMINE_COMMON_MAGIC[0],
        LIMINE_COMMON_MAGIC[1],
        LIMINE_FRAMEBUFFER_MAGIC[0],
        LIMINE_FRAMEBUFFER_MAGIC[1],
    ],
    revision: 0,
    response: core::ptr::null_mut(),
};

/// Memory map request
#[used]
#[unsafe(link_section = ".limine_reqs")]
pub static MEMMAP_REQUEST: LimineMemmapRequest = LimineMemmapRequest {
    id: [
        LIMINE_COMMON_MAGIC[0],
        LIMINE_COMMON_MAGIC[1],
        LIMINE_MEMMAP_MAGIC[0],
        LIMINE_MEMMAP_MAGIC[1],
    ],
    revision: 0,
    response: core::ptr::null_mut(),
};

// Limine writes to these at boot time (before kernel starts) — thread safety
unsafe impl Sync for LimineFramebufferRequest {}
unsafe impl Sync for LimineMemmapRequest {}

// ─── Public API ────────────────────────────────────────────────────────

static FRAMEBUFFER_READY: AtomicBool = AtomicBool::new(false);

/// Check if Limine provided a framebuffer
pub fn has_framebuffer() -> bool {
    // Safety: Limine writes the response pointer before jumping to kernel
    let resp = unsafe { &*FRAMEBUFFER_REQUEST.response };
    !FRAMEBUFFER_REQUEST.response.is_null() && resp.framebuffer_count > 0
}

/// Get the first Limine framebuffer
pub fn get_framebuffer() -> Option<&'static LimineFramebuffer> {
    if FRAMEBUFFER_REQUEST.response.is_null() {
        return None;
    }
    let resp = unsafe { &*FRAMEBUFFER_REQUEST.response };
    if resp.framebuffer_count == 0 || resp.framebuffers.is_null() {
        return None;
    }
    let fb = unsafe { &**resp.framebuffers };
    if fb.address.is_null() {
        return None;
    }
    Some(fb)
}

/// Initialize Limine framebuffer console
pub fn init_framebuffer() -> bool {
    if let Some(fb) = get_framebuffer() {
        crate::arch::fb_console::init(
            fb.address as usize,
            fb.pitch as u32,
            fb.width as u32,
            fb.height as u32,
            fb.bpp as u8,
        );
        crate::console_println!(
            "[limine] FB at {:#x} {}x{} pitch={} bpp={}",
            fb.address as usize,
            fb.width,
            fb.height,
            fb.pitch,
            fb.bpp
        );
        FRAMEBUFFER_READY.store(true, Ordering::Relaxed);
        true
    } else {
        false
    }
}

pub fn is_ready() -> bool {
    FRAMEBUFFER_READY.load(Ordering::Relaxed)
}

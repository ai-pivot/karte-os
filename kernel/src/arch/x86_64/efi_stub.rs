//! UEFI GOP stub — captures GOP framebuffer, calls ExitBootServices,
//! then loads the kernel and jumps to it with GOP info.
//! 
//! Build: cargo build -p karte-os-efi-stub --target x86_64-unknown-uefi
//! Output: target/x86_64-unknown-uefi/release/karte-os-efi-stub.efi
//! 
//! References:
//! - bootloader crate: https://docs.rs/crate/bootloader/0.10.9
//! - OS Experiment in Rust: https://blog.malware.re/2023/11/12/rust-os-part3/
//! - uefi crate: https://docs.rs/uefi-rs

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::arch::asm;

// ─── UEFI types (no uefi crate dependency — pure freestanding) ─────────

type EfiHandle = *const core::ffi::c_void;

#[repr(C)]
struct EfiGuid { data1: u32, data2: u16, data3: u16, data4: [u8; 8] }

#[repr(C)]
struct EfiTableHeader { signature: u64, revision: u32, header_size: u32, crc32: u32, _reserved: u32 }

// Simplified EFI Boot Services — we only need locate_protocol + exit_boot_services
#[repr(C)]
struct EfiBootServices {
    hdr: EfiTableHeader,
    // offset 24: raise_tpl, restore_tpl
    _pad0: [u8; 16],
    // offset 40: allocate_pages(8), free_pages(8)
    _pad1: [u8; 16],
    // offset 56: get_memory_map(8), allocate_pool(8)
    _pad2: [u8; 16],
    // offset 72: free_pool(8), create_event(8), set_timer(8)
    _pad3: [u8; 24],
    // offset 96: wait_for_event(8), signal_event(8), close_event(8), check_event(8)
    _pad4: [u8; 32],
    // offset 128: install_protocol_interface(8), reinstall(8), uninstall(8)
    _pad5: [u8; 24],
    // offset 152: handle_protocol(8), reserved(8)
    _pad6: [u8; 16],
    // offset 168: register_protocol_notify(8), locate_handle(8)
    _pad7: [u8; 16],
    // offset 184: locate_device_path(8), install_configuration_table(8)
    _pad8: [u8; 16],
    // offset 200: image_load, image_start, exit, image_unload
    _pad9: [u8; 32],
    // offset 232: exit_boot_services(8)
    exit_boot_services: unsafe extern "efiapi" fn(EfiHandle, usize) -> usize,
    // offset 240: get_next_monotonic_count, stall, set_watchdog_timer
    _pad10: [u8; 24],
    // offset 264: connect_controller, disconnect_controller
    _pad11: [u8; 16],
    // offset 280: open_protocol, close_protocol, open_protocol_information
    _pad12: [u8; 24],
    // offset 304: protocols_per_handle, locate_handle_buffer
    _pad13: [u8; 16],
    // offset 320: locate_protocol(8)
    locate_protocol: unsafe extern "efiapi" fn(*const EfiGuid, *const core::ffi::c_void, *mut *const core::ffi::c_void) -> usize,
    // ... rest
    _pad_end: [u8; 0],
}

#[repr(C)]
struct EfiSystemTable {
    hdr: EfiTableHeader,
    firmware_vendor: *const u16,
    firmware_revision: u32,
    _pad0: u32,
    console_in_handle: EfiHandle,
    con_in: *const core::ffi::c_void,
    console_out_handle: EfiHandle,
    con_out: *const core::ffi::c_void,
    std_err_handle: EfiHandle,
    std_err: *const core::ffi::c_void,
    runtime_services: *const core::ffi::c_void,
    boot_services: *const EfiBootServices,
    num_table_entries: u64,
    config_table: *const core::ffi::c_void,
}

// ─── GOP structures ────────────────────────────────────────────────────

#[repr(C)]
struct EfiGopModeInfo {
    version: u32,
    hres: u32,
    vres: u32,
    pixel_format: u32,
    pixel_info: u32,
    scanline: u32,
}

#[repr(C)]
struct EfiGopMode {
    max_mode: u32,
    mode: u32,
    info: *const EfiGopModeInfo,
    size_of_info: u64,
    fb_base: u64,
    fb_size: u64,
}

#[repr(C)]
struct EfiGop {
    query_mode: usize,
    set_mode: usize,
    blt: usize,
    mode: *const EfiGopMode,
}

const GOP_GUID: EfiGuid = EfiGuid {
    data1: 0x9042a9de, data2: 0x23dc, data3: 0x4a38,
    data4: [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
};

const EFI_SUCCESS: usize = 0;

// Kernel is embedded in the stub — the kernel binary is "included" at build time
// This way the stub + kernel = single EFI file, no filesystem loading needed
extern "C" {
    static KERNEL_START: u8;
    static KERNEL_END: u8;
}

// ─── Boot info structure passed to kernel at a fixed address ───────────

const BOOT_INFO_ADDR: usize = 0x10000;
const KERNEL_LOAD_ADDR: usize = 0x100000; // 1MB, standard kernel load address

#[repr(C)]
struct BootInfo {
    magic: u32,              // 0x544F4F42 "BOOT"
    has_fb: u32,
    fb_addr: u64,
    fb_size: u64,
    fb_width: u32,
    fb_height: u32,
    fb_stride: u32,          // bytes per scanline
    fb_format: u32,          // 1 = BGRx 32bpp
}

// ─── Entry point ──────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(_handle: EfiHandle, st_ptr: *const EfiSystemTable) -> ! {
    let st = unsafe { &*st_ptr };
    let bs = unsafe { &*st.boot_services };

    // Step 1: Capture GOP framebuffer
    let bi = unsafe { &mut *(BOOT_INFO_ADDR as *mut BootInfo) };
    bi.magic = 0x544F4F42; // "BOOT"
    bi.has_fb = 0;

    let mut gop: *const core::ffi::c_void = core::ptr::null();
    let status = unsafe { (bs.locate_protocol)(&GOP_GUID, core::ptr::null(), &mut gop) };

    if status == EFI_SUCCESS && !gop.is_null() {
        let g = unsafe { &*(gop as *const EfiGop) };
        if !g.mode.is_null() {
            let m = unsafe { &*g.mode };
            if !m.info.is_null() {
                let inf = unsafe { &*m.info };
                bi.has_fb = 1;
                bi.fb_addr = m.fb_base;
                bi.fb_size = m.fb_size;
                bi.fb_width = inf.hres;
                bi.fb_height = inf.vres;
                bi.fb_stride = inf.scanline * 4; // 32bpp = 4 bytes/pixel
                bi.fb_format = 1;
            }
        }
    }

    // Step 2: Call ExitBootServices
    let _ = unsafe { (bs.exit_boot_services)(_handle, 0) };

    // Step 3: Copy kernel to load address
    let kernel_size = unsafe {
        (&KERNEL_END as *const u8).offset_from(&KERNEL_START as *const u8) as usize
    };
    if kernel_size > 0 {
        unsafe {
            let src = &KERNEL_START as *const u8;
            let dst = KERNEL_LOAD_ADDR as *mut u8;
            for i in 0..kernel_size {
                *dst.add(i) = *src.add(i);
            }
        }
    }

    // Step 4: Jump to kernel (multiboot2 entry)
    // The kernel expects: magic in EAX, multiboot2 info ptr in EBX
    // We pass magic=0x36d76289, info=0 (kernel handles framebuffer via BootInfo)
    let kernel_entry: extern "C" fn(u32, usize) -> ! = unsafe {
        core::mem::transmute(KERNEL_LOAD_ADDR)
    };
    kernel_entry(0x36d76289, 0);
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop { unsafe { asm!("hlt"); } }
}

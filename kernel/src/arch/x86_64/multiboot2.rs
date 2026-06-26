// kernel/src/arch/x86_64/multiboot2.rs — Multiboot2 info parsing
//
// Parses the Multiboot2 information structure passed by GRUB to determine
// available physical memory. This is the proper way to get RAM size on x86_64.

/// Multiboot2 tag types
const TAG_TYPE_END: u32 = 0;
const TAG_TYPE_MMAP: u32 = 6;
const TAG_TYPE_BASIC_MEMINFO: u32 = 4;
const TAG_TYPE_FRAMEBUFFER: u32 = 8;
const TAG_TYPE_EFI_SYSTAB64: u32 = 12;

#[repr(C)]
struct MbiHeader {
    total_size: u32,
    _reserved: u32,
}

#[repr(C)]
struct TagHeader {
    tag_type: u32,
    size: u32,
}

#[repr(C)]
struct BasicMeminfoTag {
    header: TagHeader,
    mem_lower: u32,
    mem_upper: u32,
}

/// Multiboot2 framebuffer tag (type 8) — GOP info from GRUB
#[repr(C)]
struct FramebufferTag {
    header: TagHeader,
    addr: u64,
    pitch: u32,
    width: u32,
    height: u32,
    bpp: u8,
    fb_type: u8, // 0=indexed, 1=RGB, 2=EGA text
    _reserved: u16,
    // RGB color info follows when fb_type == 1
}

/// Parsed GOP framebuffer info
pub struct FramebufferInfo {
    pub addr: usize,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub bpp: u8,
}

/// Parse multiboot2 info and return (mem_lower_kb, mem_upper_kb, framebuffer, efi_st_ptr).
/// Uses memory map tag (type 6) if available; falls back to basic meminfo (type 4).
pub fn parse_mbi(mbi_addr: usize) -> (u32, u32, Option<FramebufferInfo>, Option<usize>) {
    let mut mem_lower = 0u32;
    let mut mem_upper = 0u32;
    let mut mem_from_map: u64 = 0;
    let mut fb: Option<FramebufferInfo> = None;
    let mut efi_st: Option<usize> = None;

    if mbi_addr == 0 {
        return (0, 0, None, None);
    }

    let mbi = unsafe { &*(mbi_addr as *const MbiHeader) };
    let total_size = mbi.total_size as usize;
    let mut offset = 8usize;

    while offset + 8 <= total_size {
        let tag = unsafe { &*((mbi_addr + offset) as *const TagHeader) };
        if tag.tag_type == TAG_TYPE_END {
            break;
        }

        if tag.tag_type == TAG_TYPE_BASIC_MEMINFO {
            let t = unsafe { &*((mbi_addr + offset) as *const BasicMeminfoTag) };
            mem_lower = t.mem_lower;
            mem_upper = t.mem_upper;
        }

        // Memory map tag: accumulate available RAM
        if tag.tag_type == TAG_TYPE_MMAP {
            let entry_size =
                unsafe { core::ptr::read_volatile((mbi_addr + offset + 8) as *const u32) } as usize;
            let entry_version =
                unsafe { core::ptr::read_volatile((mbi_addr + offset + 12) as *const u32) };
            if entry_version == 0 && entry_size >= 20 {
                let data_start = mbi_addr + offset + 16;
                let data_end = mbi_addr + offset + tag.size as usize;
                let mut pos = data_start;
                while pos + entry_size <= data_end {
                    let base = unsafe { core::ptr::read_volatile(pos as *const u64) };
                    let len = unsafe { core::ptr::read_volatile((pos + 8) as *const u64) };
                    let mtype = unsafe { core::ptr::read_volatile((pos + 16) as *const u32) };
                    // Type 1 = available RAM
                    if mtype == 1 {
                        mem_from_map += len;
                    }
                    pos += entry_size;
                }
            }
        }

        if tag.tag_type == TAG_TYPE_FRAMEBUFFER && fb.is_none() {
            let t = unsafe { &*((mbi_addr + offset) as *const FramebufferTag) };
            if t.fb_type == 1 && t.addr != 0 {
                fb = Some(FramebufferInfo {
                    addr: t.addr as usize,
                    pitch: t.pitch,
                    width: t.width,
                    height: t.height,
                    bpp: t.bpp,
                });
            }
        }

        if tag.tag_type == TAG_TYPE_EFI_SYSTAB64 && efi_st.is_none() {
            let ptr = unsafe { core::ptr::read_volatile((mbi_addr + offset + 8) as *const u64) };
            if ptr != 0 {
                efi_st = Some(ptr as usize);
            }
        }

        let next = (offset + tag.size as usize + 7) & !7;
        if next <= offset {
            break;
        }
        offset = next;
    }

    // Use memory map total if available (it's more accurate on UEFI)
    if mem_from_map > 0 {
        // Convert bytes to KB for mem_upper
        let map_kb = (mem_from_map / 1024) as u32;
        if map_kb > 1024 {
            // more than 1MB
            mem_upper = map_kb - 1024; // minus the first 1MB
        } else {
            mem_upper = map_kb;
        }
    }

    (mem_lower, mem_upper, fb, efi_st)
}

// EFI structures for GOP lookup
#[repr(C)]
struct EfiGuid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// Try to extract GOP framebuffer from EFI System Table.
pub fn gop_from_efi(efi_system_table: usize) -> Option<FramebufferInfo> {
    let gop_guid = EfiGuid {
        data1: 0x9042a9de,
        data2: 0x23dc,
        data3: 0x4a38,
        data4: [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
    };

    unsafe {
        let st = efi_system_table as *const u8;
        // EFI System Table: Hdr(24) + fields... + NumEntries@104 + ConfigTable@112
        let num_entries = core::ptr::read_volatile(st.add(104) as *const u64) as usize;
        let ct_ptr = core::ptr::read_volatile(st.add(112) as *const u64) as usize;

        crate::console_println!(
            "[gop-efi] st={:#x} entries={} ct={:#x}",
            efi_system_table,
            num_entries,
            ct_ptr
        );

        if ct_ptr == 0 || num_entries == 0 {
            return None;
        }

        for i in 0..num_entries {
            let entry = (ct_ptr + i * 16) as *const u8;
            let guid = &*(entry as *const EfiGuid);
            if guid.data1 == gop_guid.data1
                && guid.data2 == gop_guid.data2
                && guid.data3 == gop_guid.data3
                && guid.data4 == gop_guid.data4
            {
                crate::console_println!("[gop-efi] Found GOP GUID at entry {}", i);
                let gop_ptr = core::ptr::read_volatile(entry.add(16) as *const u64) as usize;
                if gop_ptr == 0 {
                    continue;
                }
                // GOP protocol: QueryMode(8)+SetMode(8)+Blt(8)+*Mode(8)@24
                let mode_ptr = core::ptr::read_volatile((gop_ptr + 24) as *const u64) as usize;
                if mode_ptr == 0 {
                    continue;
                }
                // GopMode: Max(4)+Mode(4)+*Info(8)+SizeOfInfo(8)+*FbBase(8)@24+FbSize(8)@32
                let fb_base = core::ptr::read_volatile((mode_ptr + 24) as *const u64) as usize;
                let info_ptr = core::ptr::read_volatile((mode_ptr + 8) as *const u64) as usize;
                if info_ptr == 0 || fb_base == 0 {
                    continue;
                }
                let hres = core::ptr::read_volatile(info_ptr as *const u32);
                let vres = core::ptr::read_volatile((info_ptr + 4) as *const u32);
                let scanline = core::ptr::read_volatile((info_ptr + 12) as *const u32);
                crate::console_println!(
                    "[gop-efi] fb={:#x} {}x{} scanline={}",
                    fb_base,
                    hres,
                    vres,
                    scanline
                );
                return Some(FramebufferInfo {
                    addr: fb_base,
                    pitch: scanline * 4,
                    width: hres,
                    height: vres,
                    bpp: 32,
                });
            }
        }
        crate::console_println!("[gop-efi] GOP GUID not found in {} entries", num_entries);
    }
    None
}

/// Legacy: parse just memory size
pub fn parse_memory_size(mbi_addr: usize) -> (u32, u32) {
    let (l, u, _, _) = parse_mbi(mbi_addr);
    (l, u)
}

/// Check if a framebuffer was found (from multiboot2)
pub fn framebuffer_info(mbi_addr: usize) -> Option<FramebufferInfo> {
    parse_mbi(mbi_addr).2
}

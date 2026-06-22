// kernel/src/arch/x86_64/multiboot2.rs — Multiboot2 info parsing
//
// Parses the Multiboot2 information structure passed by GRUB to determine
// available physical memory. This is the proper way to get RAM size on x86_64.

/// Multiboot2 tag types
const TAG_TYPE_END: u32 = 0;
const TAG_TYPE_BASIC_MEMINFO: u32 = 4;
const TAG_TYPE_FRAMEBUFFER: u32 = 8;

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

/// Parse multiboot2 info and return (mem_lower_kb, mem_upper_kb, framebuffer).
pub fn parse_mbi(mbi_addr: usize) -> (u32, u32, Option<FramebufferInfo>) {
    let mut mem_lower = 0u32;
    let mut mem_upper = 0u32;
    let mut fb: Option<FramebufferInfo> = None;

    if mbi_addr == 0 {
        return (0, 0, None);
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

        let next = (offset + tag.size as usize + 7) & !7;
        if next <= offset {
            break;
        }
        offset = next;
    }

    (mem_lower, mem_upper, fb)
}

/// Legacy: parse just memory size
pub fn parse_memory_size(mbi_addr: usize) -> (u32, u32) {
    let (l, u, _) = parse_mbi(mbi_addr);
    (l, u)
}

/// Check if a framebuffer was found
pub fn framebuffer_info(mbi_addr: usize) -> Option<FramebufferInfo> {
    parse_mbi(mbi_addr).2
}

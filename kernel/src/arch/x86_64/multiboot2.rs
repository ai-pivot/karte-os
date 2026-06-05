// kernel/src/arch/x86_64/multiboot2.rs — Multiboot2 info parsing
//
// Parses the Multiboot2 information structure passed by GRUB to determine
// available physical memory. This is the proper way to get RAM size on x86_64.

/// Multiboot2 tag types we care about
const TAG_TYPE_END: u32 = 0;
const TAG_TYPE_BASIC_MEMINFO: u32 = 4;

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
    mem_lower: u32, // KB of memory below 640KB
    mem_upper: u32, // KB of memory above 1MB
}

/// Parse multiboot2 info and return total usable memory size in bytes.
///
/// Returns (mem_lower_kb, mem_upper_kb) from the basic memory info tag.
/// If no valid tag is found, returns (0, 0).
pub fn parse_memory_size(mbi_addr: usize) -> (u32, u32) {
    if mbi_addr == 0 {
        return (0, 0);
    }

    let mbi = unsafe { &*(mbi_addr as *const MbiHeader) };
    let total_size = mbi.total_size as usize;
    let mut offset = 8usize; // skip header

    while offset + 8 <= total_size {
        let tag = unsafe { &*((mbi_addr + offset) as *const TagHeader) };

        if tag.tag_type == TAG_TYPE_END {
            break;
        }

        if tag.tag_type == TAG_TYPE_BASIC_MEMINFO {
            let meminfo = unsafe { &*((mbi_addr + offset) as *const BasicMeminfoTag) };
            return (meminfo.mem_lower, meminfo.mem_upper);
        }

        // Tags are 8-byte aligned
        let next = (offset + tag.size as usize + 7) & !7;
        if next <= offset {
            break; // prevent infinite loop
        }
        offset = next;
    }

    (0, 0)
}

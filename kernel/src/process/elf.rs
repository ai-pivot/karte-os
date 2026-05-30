//! Minimal ELF parser for RV64 executables.
//!
//! Only supports static, non-PIE, little-endian RISC-V 64-bit ELF files.

/// ELF magic: 0x7f 'E' 'L' 'F'
const ELF_MAGIC: u32 = 0x464c457f;

/// RISC-V 64-bit
#[cfg(target_arch = "riscv64")]
const EM_MACHINE: u16 = 243;

/// x86_64
#[cfg(target_arch = "x86_64")]
const EM_MACHINE: u16 = 62; // EM_X86_64

/// PT_LOAD segment type
const PT_LOAD: u32 = 1;

/// ELF header (64-bit)
#[repr(C)]
pub struct ElfHeader {
    pub ident: [u8; 16],
    pub r#type: u16,
    pub machine: u16,
    pub version: u32,
    pub entry: u64,
    pub phoff: u64,
    pub shoff: u64,
    pub flags: u32,
    pub ehsize: u16,
    pub phentsize: u16,
    pub phnum: u16,
    pub shentsize: u16,
    pub shnum: u16,
    pub shstrndx: u16,
}

/// Program header (64-bit)
#[repr(C)]
pub struct ProgramHeader {
    pub ptype: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub paddr: u64,
    pub file_size: u64,
    pub mem_size: u64,
    pub align: u64,
}

/// A loadable segment from the ELF file
#[derive(Debug, Clone)]
pub struct Segment {
    pub vaddr: usize,
    pub offset: usize,
    pub file_size: usize,
    pub mem_size: usize,
    pub flags: usize,
}

/// Parsed ELF file
pub struct ElfFile<'a> {
    pub entry: usize,
    pub loadable_segments: alloc::vec::Vec<Segment>,
    _data: core::marker::PhantomData<&'a [u8]>,
}

impl<'a> ElfFile<'a> {
    /// Parse an ELF file from raw bytes.
    pub fn parse(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < core::mem::size_of::<ElfHeader>() {
            return Err("ELF: too small");
        }

        let header: &ElfHeader = unsafe { &*(data.as_ptr() as *const ElfHeader) };

        // Verify magic
        let magic = u32::from_le_bytes(header.ident[0..4].try_into().unwrap());
        if magic != ELF_MAGIC {
            return Err("ELF: bad magic");
        }

        // Verify 64-bit, little-endian
        if header.ident[4] != 2 {
            return Err("ELF: not 64-bit");
        }
        if header.ident[5] != 1 {
            return Err("ELF: not little-endian");
        }

        // Verify machine type
        if u16::from_le(header.machine) != EM_MACHINE {
            return Err("ELF: wrong machine type");
        }

        let entry = u64::from_le(header.entry) as usize;
        let phoff = u64::from_le(header.phoff) as usize;
        let phentsize = u16::from_le(header.phentsize) as usize;
        let phnum = u16::from_le(header.phnum) as usize;

        let mut loadable_segments = alloc::vec::Vec::new();

        for i in 0..phnum {
            let ph_offset = phoff + i * phentsize;
            if ph_offset + phentsize > data.len() {
                return Err("ELF: program header out of bounds");
            }

            let ph: &ProgramHeader =
                unsafe { &*(data.as_ptr().add(ph_offset) as *const ProgramHeader) };

            if u32::from_le(ph.ptype) == PT_LOAD {
                loadable_segments.push(Segment {
                    vaddr: u64::from_le(ph.vaddr) as usize,
                    offset: u64::from_le(ph.offset) as usize,
                    file_size: u64::from_le(ph.file_size) as usize,
                    mem_size: u64::from_le(ph.mem_size) as usize,
                    flags: u32::from_le(ph.flags) as usize,
                });
            }
        }

        if loadable_segments.is_empty() {
            return Err("ELF: no loadable segments");
        }

        Ok(Self {
            entry,
            loadable_segments,
            _data: core::marker::PhantomData,
        })
    }
}

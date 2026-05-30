//! VirtIO Legacy PCI block device driver for x86_64.
//!
//! Uses I/O port BAR to access VirtIO Legacy registers.
//! Legacy mode: BAR0 is I/O space, registers at fixed offsets.

use x86_64::instructions::port::Port;

const SECTOR_SIZE: usize = 512;

// VirtIO Legacy register offsets
const REG_MAGIC: u16 = 0x00;
const REG_VERSION: u16 = 0x04;
const REG_DEVID: u16 = 0x08;
const REG_FEATURES: u16 = 0x10;
const REG_DRV_FEAT: u16 = 0x20;
const REG_PFNSZ: u16 = 0x28;
const REG_QSEL: u16 = 0x30;
const REG_QMAX: u16 = 0x34;
const REG_QNUM: u16 = 0x38;
const REG_QALIGN: u16 = 0x3c;
const REG_QPFN: u16 = 0x40;
const REG_STATUS: u16 = 0x70;
const REG_CONFIG: u16 = 0x80;
const REG_QNOTIFY: u16 = 0x50;

const STAT_ACK: u32 = 1;
const STAT_DRIVER: u32 = 2;
const STAT_FEAT_OK: u32 = 8;
const STAT_DRV_OK: u32 = 4;

const BLK_T_IN: u32 = 0;
const BLK_T_OUT: u32 = 1;
const BLK_S_OK: u8 = 0;

const QSZ: usize = 64;
const PAGE: usize = 4096;
const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

#[repr(C)]
struct BlkReq {
    kind: u32,
    _rsvd: u32,
    sector: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

static mut BLK: Option<BlkDev> = None;

/// Get reference to BLK (unsafe, only call from single-threaded kernel)
unsafe fn blk_ref() -> Option<&'static BlkDev> {
    unsafe { (*core::ptr::addr_of_mut!(BLK)).as_ref() }
}

struct BlkDev {
    io_base: u16,
    qmem: usize,
    capacity: u64,
}

fn r32(b: u16, o: u16) -> u32 {
    unsafe { Port::<u32>::new(b + o).read() }
}
fn w32(b: u16, o: u16, v: u32) {
    unsafe { Port::<u32>::new(b + o).write(v) }
}
fn aup(v: usize, a: usize) -> usize {
    (v + a - 1) & !(a - 1)
}

/// Initialize VirtIO block device at I/O port base.
pub fn init(io_base: u16) -> Result<(), &'static str> {
    let magic = r32(io_base, REG_MAGIC);
    let version = r32(io_base, REG_VERSION);
    let devid = r32(io_base, REG_DEVID);
    crate::console_println!(
        "[virtio-blk] I/O base={:#x}: magic={:#x} ver={:#x} devid={:#x}",
        io_base,
        magic,
        version,
        devid
    );
    if magic != 0x74726976 {
        return Err("bad magic");
    }
    if devid != 2 {
        return Err("not blk");
    }

    let cap_lo = r32(io_base, REG_CONFIG);
    let cap_hi = r32(io_base, REG_CONFIG + 4);
    let capacity = ((cap_hi as u64) << 32) | (cap_lo as u64);

    w32(io_base, REG_DRV_FEAT, 0);
    w32(io_base, REG_STATUS, 0);
    w32(io_base, REG_STATUS, STAT_ACK);
    w32(io_base, REG_STATUS, STAT_ACK | STAT_DRIVER);
    w32(io_base, REG_STATUS, STAT_ACK | STAT_DRIVER | STAT_FEAT_OK);
    w32(
        io_base,
        REG_STATUS,
        STAT_ACK | STAT_DRIVER | STAT_FEAT_OK | STAT_DRV_OK,
    );

    w32(io_base, REG_QSEL, 0);
    let qmax = r32(io_base, REG_QMAX) as usize;
    if qmax == 0 {
        return Err("no vq");
    }
    let qs = core::cmp::min(qmax, QSZ);

    let desc_sz = qs * core::mem::size_of::<Desc>();
    let total = aup(
        desc_sz + aup(4 + qs * 2 + 2, 4) + aup(4 + qs * 8 + 2, 4),
        PAGE,
    ) + PAGE * 3;
    let np = (total + PAGE - 1) / PAGE;
    let base = crate::mm::pmm::alloc_frame().ok_or("OOM")?;
    for _ in 1..np {
        crate::mm::pmm::alloc_frame().ok_or("OOM")?;
    }
    unsafe {
        core::ptr::write_bytes(base as *mut u8, 0, np * PAGE);
    }

    let avail_off = aup(desc_sz, 4);
    unsafe {
        core::ptr::write((base + avail_off + 2) as *mut u16, 0u16);
    }

    w32(io_base, REG_PFNSZ, PAGE as u32);
    w32(io_base, REG_QNUM, qs as u32);
    w32(io_base, REG_QALIGN, PAGE as u32);
    w32(io_base, REG_QPFN, (base >> 12) as u32);

    crate::console_println!(
        "[virtio-blk] OK: {} sectors, {} MB, io={:#x}",
        capacity,
        capacity * 512 / (1024 * 1024),
        io_base
    );

    unsafe {
        core::ptr::addr_of_mut!(BLK).write(Some(BlkDev {
            io_base,
            qmem: base,
            capacity,
        }));
    }
    Ok(())
}

/// Read one 512-byte block.
pub fn read_block(block_id: usize, buf: &mut [u8]) -> Result<(), &'static str> {
    let b = unsafe { blk_ref().ok_or("no blk")? };
    let (base, io) = (b.qmem, b.io_base);

    let desc_sz = QSZ * core::mem::size_of::<Desc>();
    let av = aup(desc_sz, 4);
    let uv = av + aup(4 + QSZ * 2 + 2, 4);
    let ro = uv + aup(4 + QSZ * 8 + 2, 4);
    let do_ = ro + PAGE;
    let so = do_ + PAGE;

    unsafe {
        let req = (base + ro) as *mut BlkReq;
        core::ptr::write(
            req,
            BlkReq {
                kind: BLK_T_IN,
                _rsvd: 0,
                sector: block_id as u64,
            },
        );
        core::ptr::write((base + so) as *mut u8, 0xffu8);

        let desc = core::slice::from_raw_parts_mut(base as *mut Desc, QSZ);
        desc[0] = Desc {
            addr: (base + ro) as u64,
            len: 16,
            flags: DESC_F_NEXT,
            next: 1,
        };
        desc[1] = Desc {
            addr: (base + do_) as u64,
            len: SECTOR_SIZE as u32,
            flags: DESC_F_NEXT | DESC_F_WRITE,
            next: 2,
        };
        desc[2] = Desc {
            addr: (base + so) as u64,
            len: 1,
            flags: DESC_F_WRITE,
            next: 0,
        };

        let aidx = core::ptr::read((base + av + 2) as *const u16);
        let idx = (aidx as usize) % QSZ;
        core::ptr::write((base + av + 4 + idx * 2) as *mut u16, 0u16);
        core::ptr::write((base + av + 2) as *mut u16, aidx.wrapping_add(1));
    }

    w32(io, REG_QNOTIFY, 0);

    for _ in 0..10_000_000 {
        let st = unsafe { core::ptr::read((base + so) as *const u8) };
        if st != 0xff {
            if st == BLK_S_OK {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (base + do_) as *const u8,
                        buf.as_mut_ptr(),
                        SECTOR_SIZE,
                    )
                };
                return Ok(());
            }
            return Err("blk read err");
        }
        core::hint::spin_loop();
    }
    Err("blk timeout")
}

/// Write one 512-byte block.
pub fn write_block(block_id: usize, buf: &[u8]) -> Result<(), &'static str> {
    let b = unsafe { blk_ref().ok_or("no blk")? };
    let (base, io) = (b.qmem, b.io_base);

    let desc_sz = QSZ * core::mem::size_of::<Desc>();
    let av = aup(desc_sz, 4);
    let uv = av + aup(4 + QSZ * 2 + 2, 4);
    let ro = uv + aup(4 + QSZ * 8 + 2, 4);
    let do_ = ro + PAGE;
    let so = do_ + PAGE;

    unsafe {
        core::ptr::copy_nonoverlapping(buf.as_ptr(), (base + do_) as *mut u8, SECTOR_SIZE);
        let req = (base + ro) as *mut BlkReq;
        core::ptr::write(
            req,
            BlkReq {
                kind: BLK_T_OUT,
                _rsvd: 0,
                sector: block_id as u64,
            },
        );
        core::ptr::write((base + so) as *mut u8, 0xffu8);

        let desc = core::slice::from_raw_parts_mut(base as *mut Desc, QSZ);
        desc[0] = Desc {
            addr: (base + ro) as u64,
            len: 16,
            flags: DESC_F_NEXT,
            next: 1,
        };
        desc[1] = Desc {
            addr: (base + do_) as u64,
            len: SECTOR_SIZE as u32,
            flags: DESC_F_NEXT,
            next: 2,
        };
        desc[2] = Desc {
            addr: (base + so) as u64,
            len: 1,
            flags: DESC_F_WRITE,
            next: 0,
        };

        let aidx = core::ptr::read((base + av + 2) as *const u16);
        let idx = (aidx as usize) % QSZ;
        core::ptr::write((base + av + 4 + idx * 2) as *mut u16, 0u16);
        core::ptr::write((base + av + 2) as *mut u16, aidx.wrapping_add(1));
    }

    w32(io, REG_QNOTIFY, 0);

    for _ in 0..10_000_000 {
        let st = unsafe { core::ptr::read((base + so) as *const u8) };
        if st != 0xff {
            return if st == BLK_S_OK {
                Ok(())
            } else {
                Err("blk write err")
            };
        }
        core::hint::spin_loop();
    }
    Err("blk timeout")
}

/// Get capacity in sectors.
pub fn capacity() -> Option<u64> {
    unsafe { blk_ref().map(|d| d.capacity) }
}

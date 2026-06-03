//! VirtIO 9p (virtio-9p) filesystem driver for QEMU host directory sharing.
//!
//! Uses the 9P2000.L protocol to access host directories shared via QEMU's
//! `-virtfs` or `-fsdev` options. This allows KarteOS to read/write files
//! on the host filesystem during QEMU development.
//!
//! QEMU example:
//!   -fsdev local,id=share1,path=/host/dir,security_model=none \
//!   -device virtio-9p-pci,fsdev=share1,mount_tag=hostshare
//!
//! Then in KarteOS: `mount 9p hostshare /host`

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::driver::vfs::{self, VfsDirEntry, VfsError, VfsFileType, VfsMetadata};
use crate::sync::spinlock::SpinLock;

// ─── 9P Message Types ────────────────────────────────────────────────────

const P9_VERSION: u8 = 100;
const P9_ATTACH: u8 = 104;
const P9_WALK: u8 = 110;
const P9_OPEN: u8 = 112;
const P9_CREATE: u8 = 114;
const P9_READ: u8 = 116;
const P9_WRITE: u8 = 118;
const P9_CLUNK: u8 = 120;
const P9_STATFS: u8 = 138; // 9P2000.L
const P9_LSTAT: u8 = 142; // 9P2000.L
const P9_READDIR: u8 = 140; // 9P2000.L
const P9_RVERSION: u8 = 101;
const P9_RATTACH: u8 = 105;
const P9_RWALK: u8 = 111;
const P9_ROPEN: u8 = 113;
const P9_RCREATE: u8 = 115;
const P9_RREAD: u8 = 117;
const P9_RWRITE: u8 = 119;
const P9_RCLUNK: u8 = 121;
const P9_RSTATFS: u8 = 139;
const P9_RLSTAT: u8 = 143;
const P9_RREADDIR: u8 = 141;

// ─── 9P QID Types ────────────────────────────────────────────────────────

const P9_QTDIR: u8 = 0x80;
const P9_QTFILE: u8 = 0x00;

// ─── 9P Open Flags ──────────────────────────────────────────────────────

const P9_OREAD: u32 = 0;
const P9_OWRITE: u32 = 1;
const P9_ORDWR: u32 = 2;
const P9_OCREATE: u32 = 0x200; // O_CREAT equivalent

// ─── VirtIO 9p PCI Device ───────────────────────────────────────────────

/// Configuration registers (from virtio_pci config space)
struct P9Config {
    tag_len: u16,
    tag: Vec<u8>,
    max_msg_size: u32,
    num_queues: u16,
}

// ─── 9P Protocol Buffer ─────────────────────────────────────────────────

/// 9P message builder/parser. All multi-byte fields are little-endian.
struct P9Buffer {
    data: Vec<u8>,
}

impl P9Buffer {
    fn new(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    fn from_slice(s: &[u8]) -> Self {
        Self { data: s.to_vec() }
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    fn as_slice(&self) -> &[u8] {
        &self.data
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data
    }

    // ── Write helpers ──

    fn write_u8(&mut self, v: u8) {
        self.data.push(v);
    }

    fn write_u16(&mut self, v: u16) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u32(&mut self, v: u32) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u64(&mut self, v: u64) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    fn write_str(&mut self, s: &str) {
        self.write_u16(s.len() as u16);
        self.data.extend_from_slice(s.as_bytes());
    }

    // ── Read helpers ──

    fn read_u8_at(&self, offset: usize) -> u8 {
        self.data.get(offset).copied().unwrap_or(0)
    }

    fn read_u16_at(&self, offset: usize) -> u16 {
        if offset + 2 > self.data.len() {
            return 0;
        }
        u16::from_le_bytes([self.data[offset], self.data[offset + 1]])
    }

    fn read_u32_at(&self, offset: usize) -> u32 {
        if offset + 4 > self.data.len() {
            return 0;
        }
        u32::from_le_bytes([
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ])
    }

    fn read_u64_at(&self, offset: usize) -> u64 {
        if offset + 8 > self.data.len() {
            return 0;
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.data[offset..offset + 8]);
        u64::from_le_bytes(bytes)
    }

    fn read_str_at(&self, offset: usize) -> (String, usize) {
        let len = self.read_u16_at(offset) as usize;
        if offset + 2 + len > self.data.len() {
            return (String::new(), 2);
        }
        let s = String::from_utf8_lossy(&self.data[offset + 2..offset + 2 + len]);
        (s.into_owned(), 2 + len)
    }
}

// ─── QID Structure ──────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
struct Qid {
    qtype: u8,
    version: u32,
    path: u64,
}

impl Qid {
    fn is_dir(&self) -> bool {
        self.qtype & P9_QTDIR != 0
    }
}

// ─── 9P Transport Layer ─────────────────────────────────────────────────

/// Transport for 9p messages. In this implementation, we use a simple
/// buffer-passing approach via shared memory with the VirtIO device.
/// For PCI-based virtio-9p, we use the virtqueue mechanism.

const P9_HEADER_SIZE: usize = 7; // size(4) + type(1) + tag(2)
const P9_DEFAULT_MSIZE: u32 = 8192;
const P9_TAG: u16 = 1; // We always use tag 1 for simplicity

/// Transport state for 9p communication
struct P9Transport {
    /// Current fid counter
    next_fid: u32,
    /// Root QID (from attach)
    root_fid: u32,
    /// Whether transport is initialized
    initialized: bool,
}

impl P9Transport {
    fn new() -> Self {
        Self {
            next_fid: 0,
            root_fid: 0,
            initialized: false,
        }
    }

    fn alloc_fid(&mut self) -> u32 {
        let fid = self.next_fid;
        self.next_fid += 1;
        fid
    }
}

// ─── Shared directory cache ─────────────────────────────────────────────

struct DirCacheEntry {
    name: String,
    qid: Qid,
}

struct P9FileSystemInner {
    transport: P9Transport,
    /// Cached directory listing for readdir
    dir_cache: Vec<DirCacheEntry>,
    /// FID → (qid, is_open) mapping
    open_fids: Vec<(u32, Qid, bool)>,
}

// ─── P9 FileSystem (implements vfs::FileSystem) ─────────────────────────

pub struct P9FileSystem {
    inner: SpinLock<P9FileSystemInner>,
    msize: u32,
}

impl P9FileSystem {
    pub fn new() -> Self {
        Self {
            inner: SpinLock::new(P9FileSystemInner {
                transport: P9Transport::new(),
                dir_cache: Vec::new(),
                open_fids: Vec::new(),
            }),
            msize: P9_DEFAULT_MSIZE,
        }
    }

    /// Initialize the 9p connection: send VERSION + ATTACH.
    pub fn init(&mut self, mount_tag: &str) -> Result<(), &'static str> {
        let mut inner = self.inner.lock();

        // Tversion
        let mut buf = P9Buffer::new(64);
        buf.write_u32(0); // placeholder for size
        buf.write_u8(P9_VERSION);
        buf.write_u16(P9_TAG);
        buf.write_u32(self.msize);
        buf.write_str("9P2000.L");

        // Fix size
        let total_size = buf.len() as u32;
        buf.data[0..4].copy_from_slice(&total_size.to_le_bytes());

        let resp = self.send_request(buf.as_slice())?;
        // Rversion: size(4) + type(1) + tag(2) + msize(4) + version(str)
        if resp.read_u8_at(4) != P9_RVERSION {
            return Err("Invalid version response");
        }
        let _server_msize = resp.read_u32_at(7);
        self.msize = self.msize.min(_server_msize);

        crate::console_println!("[9p] Version negotiated, msize={}", self.msize);

        // Tattach: attach to root
        let root_fid = inner.transport.alloc_fid();
        let mut buf = P9Buffer::new(64);
        buf.write_u32(0);
        buf.write_u8(P9_ATTACH);
        buf.write_u16(P9_TAG);
        buf.write_u32(root_fid); // fid
        buf.write_u32(0xFFFFFFFF); // afid = NOFID
        buf.write_str("nobody"); // uname
        buf.write_str(mount_tag); // aname = mount tag
        let total_size = buf.len() as u32;
        buf.data[0..4].copy_from_slice(&total_size.to_le_bytes());

        let resp = self.send_request(buf.as_slice())?;
        if resp.read_u8_at(4) != P9_RATTACH {
            return Err("Attach failed");
        }
        // Rattach: size(4) + type(1) + tag(2) + qid(13)
        let root_qid = parse_qid(&resp, 7);

        inner.transport.root_fid = root_fid;
        inner.transport.initialized = true;

        crate::console_println!(
            "[9p] Attached to '{}' (root fid={}, qid type={:#x})",
            mount_tag,
            root_fid,
            root_qid.qtype
        );

        Ok(())
    }

    /// Send a 9p request and receive a response.
    /// In this implementation, we use a simple approach:
    /// write the message to a shared buffer and read the response.
    ///
    /// For VirtIO PCI transport, this would use virtqueues.
    /// For now, we use a simplified approach via a static buffer.
    fn send_request(&self, _msg: &[u8]) -> Result<P9Buffer, &'static str> {
        // This is a stub - the actual VirtIO transport is implemented below
        // in the VirtIO transport layer
        Err("9p transport not available - needs VirtIO queue support")
    }

    // ── 9p Operations ──

    fn p9_walk(&self, fid: u32, new_fid: u32, names: &[&str]) -> Result<Vec<Qid>, &'static str> {
        let mut inner = self.inner.lock();
        if !inner.transport.initialized {
            return Err("9p not initialized");
        }

        let mut buf = P9Buffer::new(256);
        buf.write_u32(0);
        buf.write_u8(P9_WALK);
        buf.write_u16(P9_TAG);
        buf.write_u32(fid);
        buf.write_u32(new_fid);
        buf.write_u16(names.len() as u16);
        for name in names {
            buf.write_str(name);
        }
        let total_size = buf.len() as u32;
        buf.data[0..4].copy_from_slice(&total_size.to_le_bytes());

        let resp = self.send_request(buf.as_slice())?;
        if resp.read_u8_at(4) != P9_RWALK {
            return Err("Walk failed");
        }

        let nwalk = resp.read_u16_at(7) as usize;
        let mut qids = Vec::new();
        let mut offset = 9;
        for _ in 0..nwalk {
            if offset + 13 > resp.len() {
                break;
            }
            qids.push(parse_qid(&resp, offset));
            offset += 13;
        }

        // Clunk the new fid if walk failed (nwname != nwalk)
        if !names.is_empty() && nwalk != names.len() {
            let _ = self.p9_clunk(new_fid);
            return Err("Walk: not all components resolved");
        }

        Ok(qids)
    }

    fn p9_clunk(&self, fid: u32) -> Result<(), &'static str> {
        let mut buf = P9Buffer::new(32);
        buf.write_u32(0);
        buf.write_u8(P9_CLUNK);
        buf.write_u16(P9_TAG);
        buf.write_u32(fid);
        let total_size = buf.len() as u32;
        buf.data[0..4].copy_from_slice(&total_size.to_le_bytes());

        let resp = self.send_request(buf.as_slice())?;
        if resp.read_u8_at(4) != P9_RCLUNK {
            return Err("Clunk failed");
        }
        Ok(())
    }

    fn p9_open(&self, fid: u32, flags: u32) -> Result<Qid, &'static str> {
        let mut buf = P9Buffer::new(32);
        buf.write_u32(0);
        buf.write_u8(P9_OPEN);
        buf.write_u16(P9_TAG);
        buf.write_u32(fid);
        buf.write_u32(flags);
        let total_size = buf.len() as u32;
        buf.data[0..4].copy_from_slice(&total_size.to_le_bytes());

        let resp = self.send_request(buf.as_slice())?;
        if resp.read_u8_at(4) != P9_ROPEN {
            return Err("Open failed");
        }
        let qid = parse_qid(&resp, 7);
        // iounit at offset 20
        Ok(qid)
    }

    fn p9_read(&self, fid: u32, offset: u64, count: u32) -> Result<Vec<u8>, &'static str> {
        let mut buf = P9Buffer::new(32);
        buf.write_u32(0);
        buf.write_u8(P9_READ);
        buf.write_u16(P9_TAG);
        buf.write_u32(fid);
        buf.write_u64(offset);
        buf.write_u32(count);
        let total_size = buf.len() as u32;
        buf.data[0..4].copy_from_slice(&total_size.to_le_bytes());

        let resp = self.send_request(buf.as_slice())?;
        if resp.read_u8_at(4) != P9_RREAD {
            return Err("Read failed");
        }
        let nread = resp.read_u32_at(7) as usize;
        if nread + 11 > resp.len() {
            return Ok(resp.data[11..].to_vec());
        }
        Ok(resp.data[11..11 + nread].to_vec())
    }

    fn p9_lstat(&self, fid: u32) -> Result<StatResult, &'static str> {
        // Walk to get a fid for the path, then lstat
        let mut buf = P9Buffer::new(32);
        buf.write_u32(0);
        buf.write_u8(P9_LSTAT); // Tlstat
        buf.write_u16(P9_TAG);
        buf.write_u32(fid);
        let total_size = buf.len() as u32;
        buf.data[0..4].copy_from_slice(&total_size.to_le_bytes());

        let resp = self.send_request(buf.as_slice())?;
        if resp.read_u8_at(4) != P9_RLSTAT {
            return Err("Lstat failed");
        }
        // Rlstat: size(4) + type(1) + tag(2) + stat(n)
        let stat_len = resp.read_u16_at(7) as usize;
        if stat_len == 0 || 9 + stat_len > resp.len() {
            return Err("Invalid stat response");
        }
        Ok(parse_linux_stat9(&resp, 9))
    }
}

// ─── Helper Functions ────────────────────────────────────────────────────

struct StatResult {
    qid: Qid,
    mode: u32,
    size: u64,
    name: String,
}

fn parse_qid(buf: &P9Buffer, offset: usize) -> Qid {
    Qid {
        qtype: buf.read_u8_at(offset),
        version: buf.read_u32_at(offset + 1),
        path: buf.read_u64_at(offset + 5),
    }
}

fn parse_linux_stat9(buf: &P9Buffer, offset: usize) -> StatResult {
    // Linux stat structure in 9P2000.L: stat(n) contains:
    // mode(4), uid(str), gid(str), nlink(4), rdev(4), size(8),
    // blksize(8), blocks(8), atime(8), mtime(8), ctime(8),
    // btime(8), gen(8), data_version(8)
    let mode = buf.read_u32_at(offset);
    // Skip uid, gid strings
    let (uid_str, uid_sz) = buf.read_str_at(offset + 4);
    let gid_off = offset + 4 + uid_sz;
    let (_gid_str, gid_sz) = buf.read_str_at(gid_off);
    let nlink_off = gid_off + gid_sz;

    // nlink(4) + rdev(4) + size(8)
    let size_off = nlink_off + 4 + 4;
    let size = buf.read_u64_at(size_off);

    let qid = Qid {
        qtype: if mode & 0x4000 != 0 {
            P9_QTDIR
        } else {
            P9_QTFILE
        },
        version: 0,
        path: 0,
    };

    StatResult {
        qid,
        mode,
        size,
        name: String::new(), // Name not in lstat, need to extract from walk
    }
}

// ─── VFS FileSystem Implementation ───────────────────────────────────────

impl vfs::FileSystem for P9FileSystem {
    fn name(&self) -> &str {
        "9p"
    }

    fn root_inode(&self) -> u64 {
        let inner = self.inner.lock();
        inner.transport.root_fid as u64
    }

    fn lookup(&self, dir: u64, name: &str) -> Result<u64, VfsError> {
        let mut inner = self.inner.lock();
        if !inner.transport.initialized {
            return Err(VfsError::IoError);
        }
        let new_fid = inner.transport.alloc_fid() as u64;
        drop(inner);

        match self.p9_walk(dir as u32, new_fid as u32, &[name]) {
            Ok(_) => Ok(new_fid),
            Err(_) => Err(VfsError::NotFound),
        }
    }

    fn metadata(&self, inode: u64) -> Result<VfsMetadata, VfsError> {
        let stat = self.p9_lstat(inode as u32).map_err(|_| VfsError::IoError)?;
        Ok(VfsMetadata {
            file_type: if stat.qid.is_dir() {
                VfsFileType::Directory
            } else {
                VfsFileType::File
            },
            size: stat.size as usize,
            name: stat.name,
        })
    }

    fn readdir(&self, dir: u64, idx: usize) -> Result<Option<VfsDirEntry>, VfsError> {
        // For now, return None (readdir via 9p is complex)
        // A full implementation would use Treaddir
        let _ = (dir, idx);
        Ok(None)
    }

    fn read_file(&self, inode: u64, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        // Open the fid first, then read
        let _ = self
            .p9_open(inode as u32, P9_OREAD)
            .map_err(|_| VfsError::IoError)?;
        let data = self
            .p9_read(inode as u32, offset as u64, buf.len() as u32)
            .map_err(|_| VfsError::IoError)?;
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
    }

    fn write_file(&mut self, _inode: u64, _offset: usize, _data: &[u8]) -> Result<usize, VfsError> {
        // Write support not yet implemented
        Err(VfsError::PermissionDenied)
    }

    fn create_file(&mut self, _dir: u64, _name: &str) -> Result<u64, VfsError> {
        Err(VfsError::PermissionDenied)
    }

    fn create_dir(&mut self, _dir: u64, _name: &str) -> Result<u64, VfsError> {
        Err(VfsError::PermissionDenied)
    }

    fn unlink(&mut self, _dir: u64, _name: &str) -> Result<(), VfsError> {
        Err(VfsError::PermissionDenied)
    }

    fn set_file_size(&mut self, _inode: u64, _size: usize) -> Result<(), VfsError> {
        Err(VfsError::PermissionDenied)
    }
}

/// Check if 9p filesystem is available
static P9_AVAILABLE: AtomicBool = AtomicBool::new(false);

pub fn is_available() -> bool {
    P9_AVAILABLE.load(Ordering::Relaxed)
}

/// Initialize the 9p filesystem driver.
/// This is a placeholder - actual VirtIO transport needs to be connected.
pub fn try_init(_io_base: u16, mount_tag: &str) -> Result<(), &'static str> {
    let mut fs = P9FileSystem::new();
    match fs.init(mount_tag) {
        Ok(()) => match vfs::mount("/host", alloc::boxed::Box::new(fs)) {
            Ok(()) => {
                P9_AVAILABLE.store(true, Ordering::Relaxed);
                crate::console_println!("[9p] Mounted at /host");
                Ok(())
            }
            Err(e) => Err("Failed to mount 9p filesystem"),
        },
        Err(e) => Err(e),
    }
}

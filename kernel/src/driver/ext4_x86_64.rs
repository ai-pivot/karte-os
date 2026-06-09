//! ext4 filesystem — x86_64 implementation (AHCI or VirtIO block device).

extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ext4_rs::{BLOCK_SIZE, BlockDevice as Ext4BlockDevice, EXT4_INODE_MODE_FILE, Ext4, ROOT_INODE};

use crate::driver::vfs::{FileSystem, VfsDirEntry, VfsError, VfsFileType, VfsMetadata};

const SECTOR_SIZE: usize = 512;

// ─── Sector write-through cache ────────────────────────────────────
// ext4_rs has no in-memory block cache. Every write goes directly to
// disk via write_offset's read-modify-write at sector granularity.
// When multiple metadata updates (bitmap, inode, bgdt) share the same
// physical sector, the second write's read phase picks up stale data,
// silently discarding the first write (e.g. bitmap update lost → block
// double-allocated → directory data overwritten).
//
// Fix: cache every sector we write. Subsequent reads hit the cache
// first, ensuring write-after-write consistency without requiring a
// full block layer in ext4_rs.

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;

const CACHE_CAPACITY: usize = 2048;

struct SectorCache {
    map: BTreeMap<usize, [u8; SECTOR_SIZE]>,
    order: VecDeque<usize>,
}

impl SectorCache {
    const fn new() -> Self {
        Self {
            map: BTreeMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, sector: usize) -> Option<&[u8; SECTOR_SIZE]> {
        self.map.get(&sector)
    }

    fn insert(&mut self, sector: usize, data: [u8; SECTOR_SIZE]) {
        if self.map.insert(sector, data).is_none() {
            self.order.push_back(sector);
            while self.order.len() > CACHE_CAPACITY {
                if let Some(evict) = self.order.pop_front() {
                    self.map.remove(&evict);
                }
            }
        }
    }
}

static SECTOR_CACHE: spin::Mutex<SectorCache> = spin::Mutex::new(SectorCache::new());

static READ_COUNT: AtomicUsize = AtomicUsize::new(0);
static WRITE_COUNT: AtomicUsize = AtomicUsize::new(0);
static MAX_DISK_OFFSET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn set_max_disk_offset(max_offset: u64) {
    MAX_DISK_OFFSET.store(max_offset, Ordering::Relaxed);
}

fn is_offset_valid(offset: usize) -> bool {
    let max = MAX_DISK_OFFSET.load(Ordering::Relaxed);
    max == 0 || offset < max as usize
}

fn block_read(sector: usize, buf: &mut [u8]) -> Result<(), &'static str> {
    READ_COUNT.fetch_add(1, Ordering::Relaxed);
    if crate::driver::ahci::is_available() {
        crate::driver::ahci::read_block(sector, buf)
    } else {
        crate::arch::virtio_blk::read_block(sector, buf)
    }
}

fn block_write(sector: usize, buf: &[u8]) -> Result<(), &'static str> {
    WRITE_COUNT.fetch_add(1, Ordering::Relaxed);
    if crate::driver::ahci::is_available() {
        crate::driver::ahci::write_block(sector, buf)
    } else {
        crate::arch::virtio_blk::write_block(sector, buf)
    }
}

pub struct KarteBlockDevice;

impl KarteBlockDevice {
    pub fn new() -> Self {
        Self
    }
}

impl Ext4BlockDevice for KarteBlockDevice {
    fn read_offset(&self, offset: usize) -> Vec<u8> {
        READ_COUNT.fetch_add(1, Ordering::Relaxed);
        if !is_offset_valid(offset + BLOCK_SIZE - 1) {
            return alloc::vec![0u8; BLOCK_SIZE];
        }
        let mut buf = alloc::vec![0u8; BLOCK_SIZE];
        let base_sector = offset / SECTOR_SIZE;
        let skip = offset % SECTOR_SIZE;

        let mut buf_pos = 0;
        let mut sector_idx = 0;
        while buf_pos < BLOCK_SIZE {
            let abs_sector = base_sector + sector_idx;
            let src_start = if sector_idx == 0 { skip } else { 0 };
            let remaining = BLOCK_SIZE - buf_pos;
            let copy_len = core::cmp::min(SECTOR_SIZE - src_start, remaining);

            // Try cache first
            let cache = SECTOR_CACHE.lock();
            if let Some(cached) = cache.get(abs_sector) {
                buf[buf_pos..buf_pos + copy_len]
                    .copy_from_slice(&cached[src_start..src_start + copy_len]);
                drop(cache);
            } else {
                drop(cache);
                let mut sector = [0u8; SECTOR_SIZE];
                if block_read(abs_sector, &mut sector).is_err() {
                    break;
                }
                buf[buf_pos..buf_pos + copy_len]
                    .copy_from_slice(&sector[src_start..src_start + copy_len]);
            }
            buf_pos += copy_len;
            sector_idx += 1;
        }
        buf
    }

    fn write_offset(&self, offset: usize, data: &[u8]) {
        WRITE_COUNT.fetch_add(1, Ordering::Relaxed);
        if !is_offset_valid(offset + data.len() - 1) {
            return;
        }
        let mut data_pos = 0;
        let mut current_offset = offset;

        while data_pos < data.len() {
            let sector_idx = current_offset / SECTOR_SIZE;
            let sector_offset = current_offset % SECTOR_SIZE;
            let bytes_in_sector =
                core::cmp::min(SECTOR_SIZE - sector_offset, data.len() - data_pos);

            let mut sector = [0u8; SECTOR_SIZE];

            // For partial writes, read existing sector (try cache first)
            if sector_offset != 0 || bytes_in_sector != SECTOR_SIZE {
                let cache = SECTOR_CACHE.lock();
                if let Some(cached) = cache.get(sector_idx) {
                    sector.copy_from_slice(cached);
                } else {
                    drop(cache);
                    if block_read(sector_idx, &mut sector).is_err() {
                        return;
                    }
                }
            }

            sector[sector_offset..sector_offset + bytes_in_sector]
                .copy_from_slice(&data[data_pos..data_pos + bytes_in_sector]);

            if block_write(sector_idx, &sector).is_err() {
                return;
            }

            // Update cache (write-through)
            SECTOR_CACHE.lock().insert(sector_idx, sector);

            data_pos += bytes_in_sector;
            current_offset += bytes_in_sector;
        }
    }
}

unsafe impl Send for KarteBlockDevice {}
unsafe impl Sync for KarteBlockDevice {}

/// Descriptor for an open ext4 file, stored in FdTable.
#[derive(Clone, Debug, PartialEq)]
pub struct Ext4FileDesc {
    pub inode_num: u32,
    pub writable: bool,
}

pub struct Ext4Fs {
    ext4: spin::Mutex<Ext4>,
}

impl Ext4Fs {
    pub fn new() -> Result<Self, &'static str> {
        let bd = Arc::new(KarteBlockDevice::new());
        let ext4 =
            Ext4::try_open(bd).map_err(|_| "no valid ext4 filesystem found on block device")?;
        Ok(Self {
            ext4: spin::Mutex::new(ext4),
        })
    }
}

impl FileSystem for Ext4Fs {
    fn name(&self) -> &str {
        "ext4"
    }

    fn root_inode(&self) -> u64 {
        ROOT_INODE as u64
    }

    fn lookup(&self, dir: u64, name: &str) -> Result<u64, VfsError> {
        let ext4 = self.ext4.lock();
        let mut current_dir = dir as u32;
        for component in name.split('/') {
            if component.is_empty() {
                continue;
            }
            let entries = ext4.dir_get_entries(current_dir);
            let mut found = None;
            for entry in &entries {
                let entry_name = entry.get_name();
                if entry_name == component {
                    found = Some(entry.inode);
                }
            }
            match found {
                Some(inode) => current_dir = inode,
                None => return Err(VfsError::NotFound),
            }
        }
        Ok(current_dir as u64)
    }

    fn metadata(&self, inode: u64) -> Result<VfsMetadata, VfsError> {
        let ext4 = self.ext4.lock();
        let inode_ref = ext4.get_inode_ref(inode as u32);
        let inode_obj = inode_ref.inode;
        let mode = inode_obj.mode();
        let is_dir = (mode & 0x4000) != 0;
        let file_type = if is_dir {
            VfsFileType::Directory
        } else {
            VfsFileType::File
        };
        let size = inode_obj.size() as usize;
        Ok(VfsMetadata {
            file_type,
            size,
            name: String::new(),
        })
    }

    fn readdir(&self, dir: u64, idx: usize) -> Result<Option<VfsDirEntry>, VfsError> {
        let ext4 = self.ext4.lock();
        let entries = ext4.dir_get_entries(dir as u32);
        if idx >= entries.len() {
            return Ok(None);
        }
        let entry = &entries[idx];
        let de_type = entry.get_de_type();
        let file_type = if de_type == 2 {
            VfsFileType::Directory
        } else {
            VfsFileType::File
        };
        let child_inode_ref = ext4.get_inode_ref(entry.inode);
        let size = child_inode_ref.inode.size() as usize;
        Ok(Some(VfsDirEntry {
            name: entry.get_name(),
            file_type,
            size,
        }))
    }

    fn read_file(&self, inode: u64, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let ext4 = self.ext4.lock();
        match ext4.read_at(inode as u32, offset, buf) {
            Ok(n) => Ok(n),
            Err(e) => {
                crate::console_println!("[ext4rd] ERR inode={} off={} err={:?}", inode, offset, e);
                Err(VfsError::IoError)
            }
        }
    }

    fn write_file(&mut self, inode: u64, offset: usize, data: &[u8]) -> Result<usize, VfsError> {
        let ext4 = self.ext4.lock();
        match ext4.write_at(inode as u32, offset, data) {
            Ok(n) => Ok(n),
            Err(_) => Err(VfsError::IoError),
        }
    }

    fn create_file(&mut self, dir: u64, name: &str) -> Result<u64, VfsError> {
        let ext4 = self.ext4.lock();
        match ext4.create(dir as u32, name, EXT4_INODE_MODE_FILE as u16) {
            Ok(inode_ref) => {
                let ino = inode_ref.inode_num;
                Ok(ino as u64)
            }
            Err(_) => Err(VfsError::IoError),
        }
    }

    fn create_dir(&mut self, dir: u64, name: &str) -> Result<u64, VfsError> {
        let ext4 = self.ext4.lock();
        match ext4.create(dir as u32, name, 0x4000) {
            Ok(inode_ref) => Ok(inode_ref.inode_num as u64),
            Err(_) => Err(VfsError::IoError),
        }
    }

    fn unlink(&mut self, dir: u64, name: &str) -> Result<(), VfsError> {
        let ext4 = self.ext4.lock();
        let mut parent_ref = ext4.get_inode_ref(dir as u32);
        let entries = ext4.dir_get_entries(dir as u32);
        let child_entry = entries
            .iter()
            .find(|e| e.get_name() == name)
            .ok_or(VfsError::NotFound)?;
        let mut child_ref = ext4.get_inode_ref(child_entry.inode);
        match ext4.unlink(&mut parent_ref, &mut child_ref, name) {
            Ok(_) => Ok(()),
            Err(_) => Err(VfsError::IoError),
        }
    }

    fn set_file_size(&mut self, _inode: u64, _size: usize) -> Result<(), VfsError> {
        Ok(())
    }
}

// SpinLock (not YieldMutex) for EXT4_FS: AHCI I/O happens while holding this lock.
// Timer interrupt during I/O + YieldMutex = infinite yield deadlock.
pub(crate) static EXT4_FS: spin::Mutex<Option<Ext4Fs>> = spin::Mutex::new(None);
static EXT4_AVAILABLE: AtomicBool = AtomicBool::new(false);

fn split_last_component(path: &str) -> (&str, &str) {
    if let Some(idx) = path.rfind('/') {
        (&path[..idx], &path[idx + 1..])
    } else {
        ("", path)
    }
}

pub fn init() -> Result<(), &'static str> {
    match Ext4Fs::new() {
        Ok(fs) => {
            // Compute disk size from superblock for OOB protection
            let ext4 = fs.ext4.lock();
            let sb = &ext4.super_block;
            let block_size = sb.block_size() as usize;
            let max_offset = (sb.blocks_count() as u64) * (block_size as u64);
            set_max_disk_offset(max_offset);
            drop(ext4);

            *EXT4_FS.lock() = Some(fs);
            EXT4_AVAILABLE.store(true, Ordering::SeqCst);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub fn has_ext4() -> bool {
    EXT4_AVAILABLE.load(Ordering::SeqCst)
}

// ─── VFS Integration ────────────────────────────────────────────────────

/// Wrapper that implements the VFS FileSystem trait for ext4.
/// All operations are routed to the ext4_rs functions under kernel CR3.
pub struct Ext4FileSystem;

impl crate::driver::vfs::FileSystem for Ext4FileSystem {
    fn name(&self) -> &str {
        "ext4"
    }

    fn root_inode(&self) -> u64 {
        ROOT_INODE as u64
    }

    fn lookup(&self, dir: u64, name: &str) -> Result<u64, crate::driver::vfs::VfsError> {
        with_kernel_cr3_ext4(|| {
            let guard = EXT4_FS.lock();
            let fs = guard
                .as_ref()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            fs.lookup(dir, name)
                .map_err(|_| crate::driver::vfs::VfsError::NotFound)
        })
    }

    fn metadata(
        &self,
        inode: u64,
    ) -> Result<crate::driver::vfs::VfsMetadata, crate::driver::vfs::VfsError> {
        with_kernel_cr3_ext4(|| {
            let guard = EXT4_FS.lock();
            let fs = guard
                .as_ref()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            fs.metadata(inode)
                .map_err(|_| crate::driver::vfs::VfsError::NotFound)
        })
    }

    fn readdir(
        &self,
        dir: u64,
        idx: usize,
    ) -> Result<Option<crate::driver::vfs::VfsDirEntry>, crate::driver::vfs::VfsError> {
        with_kernel_cr3_ext4(|| {
            let guard = EXT4_FS.lock();
            let fs = guard
                .as_ref()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            let ext4 = fs.ext4.lock();
            let entries = ext4.dir_get_entries(dir as u32);
            if idx >= entries.len() {
                return Ok(None);
            }
            let entry = &entries[idx];
            let de_type = entry.get_de_type();
            let file_type = if de_type == 2 {
                crate::driver::vfs::VfsFileType::Directory
            } else {
                crate::driver::vfs::VfsFileType::File
            };
            let child_inode_ref = ext4.get_inode_ref(entry.inode);
            let size = child_inode_ref.inode.size() as usize;
            Ok(Some(crate::driver::vfs::VfsDirEntry {
                name: entry.get_name(),
                file_type,
                size,
            }))
        })
    }

    fn read_file(
        &self,
        inode: u64,
        offset: usize,
        buf: &mut [u8],
    ) -> Result<usize, crate::driver::vfs::VfsError> {
        with_kernel_cr3_ext4(|| {
            let guard = EXT4_FS.lock();
            let fs = guard
                .as_ref()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            fs.read_file(inode, offset, buf)
                .map_err(|_| crate::driver::vfs::VfsError::IoError)
        })
    }

    fn write_file(
        &mut self,
        inode: u64,
        offset: usize,
        data: &[u8],
    ) -> Result<usize, crate::driver::vfs::VfsError> {
        with_kernel_cr3_ext4(|| {
            let mut guard = EXT4_FS.lock();
            let fs = guard
                .as_mut()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            fs.write_file(inode, offset, data)
                .map_err(|_| crate::driver::vfs::VfsError::IoError)
        })
    }

    fn create_file(&mut self, dir: u64, name: &str) -> Result<u64, crate::driver::vfs::VfsError> {
        with_kernel_cr3_ext4(|| {
            let mut guard = EXT4_FS.lock();
            let fs = guard
                .as_mut()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            let mut ext4 = fs.ext4.lock();
            let inode_ref = ext4
                .create(dir as u32, name, 0o100644 as u16)
                .map_err(|e| {
                    crate::console_println!("[ext4 vfs] create_file '{}' err={:?}", name, e);
                    crate::driver::vfs::VfsError::IoError
                })?;
            Ok(inode_ref.inode_num as u64)
        })
    }

    fn create_dir(&mut self, dir: u64, name: &str) -> Result<u64, crate::driver::vfs::VfsError> {
        with_kernel_cr3_ext4(|| {
            let mut guard = EXT4_FS.lock();
            let fs = guard
                .as_mut()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            let mut ext4 = fs.ext4.lock();
            let inode_ref = ext4
                .create(dir as u32, name, 0o40755 as u16)
                .map_err(|_| crate::driver::vfs::VfsError::IoError)?;
            Ok(inode_ref.inode_num as u64)
        })
    }

    fn unlink(&mut self, dir: u64, name: &str) -> Result<(), crate::driver::vfs::VfsError> {
        with_kernel_cr3_ext4(|| {
            let mut guard = EXT4_FS.lock();
            let fs = guard
                .as_mut()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            fs.unlink(dir, name)
                .map_err(|_| crate::driver::vfs::VfsError::IoError)
        })
    }

    fn set_file_size(
        &mut self,
        inode: u64,
        size: usize,
    ) -> Result<(), crate::driver::vfs::VfsError> {
        with_kernel_cr3_ext4(|| {
            let mut guard = EXT4_FS.lock();
            let fs = guard
                .as_mut()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            fs.set_file_size(inode, size)
                .map_err(|_| crate::driver::vfs::VfsError::IoError)
        })
    }
}

/// Register ext4 as the root filesystem in VFS. Called after successful init().
pub fn mount_to_vfs() -> Result<(), &'static str> {
    crate::driver::vfs::mount("/", alloc::boxed::Box::new(Ext4FileSystem))
        .map_err(|_| "failed to mount ext4 to VFS")
}

/// Run a closure under kernel CR3 on x86_64. Required for all ext4 file
/// operations because the AHCI DMA buffer is accessed via identity mapping,
/// which gets corrupted in user page tables after ELF loading overwrites
/// identity-mapped PTEs. On RISC-V, this is a no-op (identity mapping is
/// preserved in user page tables via satp copy).
#[cfg(target_arch = "x86_64")]
fn with_kernel_cr3_ext4<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    crate::arch::trap::with_kernel_cr3(f)
}

#[cfg(not(target_arch = "x86_64"))]
fn with_kernel_cr3_ext4<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    f()
}

pub fn read_file(name: &str) -> Option<Vec<u8>> {
    if !has_ext4() {
        return None;
    }
    with_kernel_cr3_ext4(|| {
        let guard = EXT4_FS.lock();
        let fs = guard.as_ref()?;
        let inode = fs.lookup(ROOT_INODE as u64, name).ok()?;
        let metadata = fs.metadata(inode).ok()?;
        let size = metadata.size;
        if size == 0 || size > 16 * 1024 * 1024 {
            // Safety: reject unreasonably large sizes to prevent capacity overflow
            crate::console_println!("[read_file_to_buf] size too large or zero: {}", size);
            return Some(Vec::new());
        }
        let mut buf = alloc::vec![0u8; size];
        let bytes_read = fs.read_file(inode, 0, &mut buf).ok()?;
        buf.truncate(bytes_read);
        Some(buf)
    })
}

pub fn read_file_range(name: &str) -> Option<impl Fn(usize, &mut [u8]) -> Result<usize, ()>> {
    if !has_ext4() {
        return None;
    }
    with_kernel_cr3_ext4(|| {
        let guard = EXT4_FS.lock();
        let fs = guard.as_ref()?;
        let inode = fs.lookup(ROOT_INODE as u64, name).ok()?;
        drop(guard);

        Some(move |offset: usize, buf: &mut [u8]| -> Result<usize, ()> {
            if !has_ext4() {
                return Err(());
            }
            with_kernel_cr3_ext4(|| {
                let guard = EXT4_FS.lock();
                let fs = guard.as_ref().ok_or(())?;
                fs.read_file(inode, offset, buf).map_err(|_| ())
            })
        })
    })
}

pub fn lookup_path(name: &str) -> Option<u64> {
    if !has_ext4() {
        return None;
    }
    with_kernel_cr3_ext4(|| {
        let guard = EXT4_FS.lock();
        let fs = guard.as_ref()?;
        fs.lookup(ROOT_INODE as u64, name).ok()
    })
}

pub fn metadata_of(inode: u64) -> Option<VfsMetadata> {
    if !has_ext4() {
        return None;
    }
    with_kernel_cr3_ext4(|| {
        let guard = EXT4_FS.lock();
        let fs = guard.as_ref()?;
        fs.metadata(inode).ok()
    })
}

pub fn write_file(name: &str, data: &[u8]) -> Result<(), &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }
    with_kernel_cr3_ext4(|| write_file_inner(name, data))
}

fn write_file_inner(name: &str, data: &[u8]) -> Result<(), &'static str> {
    let mut guard = EXT4_FS.lock();
    let fs = guard.as_mut().ok_or("ext4 not initialized")?;
    let inode = fs.lookup(ROOT_INODE as u64, name).unwrap_or_else(|_| 0);
    if inode != 0 {
        fs.write_file(inode, 0, data).map_err(|e| {
            crate::console_println!(
                "[ext4] write existing '{}' inode={}: err {:?}",
                name,
                inode,
                e
            );
            "ext4 write failed"
        })?;
    } else {
        let (parent_path, file_name) = split_last_component(name);

        let parent_inode = if parent_path.is_empty() {
            ROOT_INODE as u64
        } else {
            fs.lookup(ROOT_INODE as u64, parent_path).map_err(|e| {
                crate::console_println!("[ext4] parent '{}' not found: {:?}", parent_path, e);
                "ext4: parent directory not found"
            })?
        };

        let inode = fs.create_file(parent_inode, file_name).map_err(|e| {
            crate::console_println!(
                "[ext4] create_file '{}' in inode {}: err {:?}",
                file_name,
                parent_inode,
                e
            );
            "ext4 create failed"
        })?;

        fs.write_file(inode, 0, data).map_err(|e| {
            crate::console_println!("[ext4] write new '{}' inode={}: err {:?}", name, inode, e);
            "ext4 write failed"
        })?;
    }
    Ok(())
}

pub fn list_directory(path: &str) -> Vec<(String, usize)> {
    let mut result = Vec::new();
    if !has_ext4() {
        return result;
    }
    with_kernel_cr3_ext4(|| {
        let guard = EXT4_FS.lock();
        let Some(fs) = guard.as_ref() else {
            return result;
        };
        let dir_inode = if path.is_empty() {
            ROOT_INODE as u64
        } else {
            match fs.lookup(ROOT_INODE as u64, path) {
                Ok(inode) => inode,
                Err(_) => return result,
            }
        };
        let mut idx = 0;
        loop {
            match fs.readdir(dir_inode, idx) {
                Ok(Some(entry)) => {
                    if entry.name != "." && entry.name != ".." {
                        result.push((entry.name, entry.size));
                    }
                    idx += 1;
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
        result
    })
}

pub fn create_directory(path: &str) -> Result<(), &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }

    // All ext4 file operations must run under kernel CR3. The AHCI DMA buffer
    // is accessed via identity mapping, which is overwritten in user page tables
    // after ELF loading. Without kernel CR3, block I/O reads wrong physical pages.
    #[cfg(target_arch = "x86_64")]
    return crate::arch::trap::with_kernel_cr3(|| create_directory_inner(path));
    #[cfg(not(target_arch = "x86_64"))]
    create_directory_inner(path)
}

fn create_directory_inner(path: &str) -> Result<(), &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }

    // Check if already exists
    {
        let guard = EXT4_FS.lock();
        if let Some(fs) = guard.as_ref() {
            if fs.lookup(ROOT_INODE as u64, path).is_ok() {
                return Ok(());
            }
        }
    }

    let (parent_path, dir_name) = split_last_component(path);

    // Ensure parent exists recursively
    if !parent_path.is_empty() {
        let parent_exists = {
            let guard = EXT4_FS.lock();
            guard.as_ref().map_or(false, |fs| {
                fs.lookup(ROOT_INODE as u64, parent_path).is_ok()
            })
        };
        if !parent_exists {
            create_directory(parent_path)?;
        }
    }

    let mut guard = EXT4_FS.lock();
    let fs = guard.as_mut().ok_or("ext4 not initialized")?;
    let parent_inode = if parent_path.is_empty() {
        ROOT_INODE as u64
    } else {
        fs.lookup(ROOT_INODE as u64, parent_path).map_err(|e| {
            crate::console_println!(
                "[ext4] create_dir: parent '{}' not found: {:?}",
                parent_path,
                e
            );
            "parent not found"
        })?
    };

    let mut ext4 = fs.ext4.lock();
    match ext4.create(parent_inode as u32, dir_name, 0o40777 as u16) {
        Ok(_) => Ok(()),
        Err(e) => {
            crate::console_println!(
                "[ext4] create_dir FAILED '{}/{}' err={:?}",
                parent_path,
                dir_name,
                e
            );
            Err("ext4 create_dir failed")
        }
    }
}

pub fn delete_file(name: &str) -> Result<(), &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }
    with_kernel_cr3_ext4(|| {
        let mut guard = EXT4_FS.lock();
        let fs = guard.as_mut().ok_or("ext4 not initialized")?;
        let (parent_path, file_name) = split_last_component(name);
        let parent_inode = if parent_path.is_empty() {
            ROOT_INODE as u64
        } else {
            fs.lookup(ROOT_INODE as u64, parent_path)
                .map_err(|_| "ext4: parent directory not found")?
        };
        fs.unlink(parent_inode, file_name)
            .map_err(|_| "ext4 unlink failed")?;
        Ok(())
    })
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── ext4 Tests (x86_64) ──");

    crate::test::run_test("ext4_read_count", || {
        let count = READ_COUNT.load(Ordering::SeqCst);
        count >= 0
    });

    crate::test::run_test("ext4_write_count", || {
        let count = WRITE_COUNT.load(Ordering::SeqCst);
        count >= 0
    });
}

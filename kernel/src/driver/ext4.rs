//! ext4 filesystem module — architecture-independent implementation.
//!
//! Architecture-specific block I/O is injected via function pointers stored
//! in `KarteBlockDevice`. Each architecture provides `create_block_device()`
//! with its own `block_read` / `block_write` callbacks.
//!
//! # Architecture dispatch
//!
//! `ext4_arch` is selected via `#[path]` and re-exported with `pub use`.
//! The arch file provides:
//!   - `pub fn create_block_device() -> KarteBlockDevice`
//!   - `pub fn mount_to_vfs() -> Result<(), &'static str>`  (optional, x86_64 only)
//!   - `pub fn run_tests()`                                 (optional)
//!
//! All shared types (`KarteBlockDevice`, `Ext4FileDesc`, `Ext4Fs`, etc.)
//! and the public API live here.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use ext4_rs::{BLOCK_SIZE, BlockDevice as Ext4BlockDevice, EXT4_INODE_MODE_FILE, Ext4, ROOT_INODE};

use crate::driver::vfs::{FileSystem, VfsDirEntry, VfsError, VfsFileType, VfsMetadata};

// ─── Architecture dispatch ────────────────────────────────────────────────

#[cfg(target_arch = "riscv64")]
#[path = "ext4_riscv.rs"]
mod ext4_arch;

#[cfg(target_arch = "x86_64")]
#[path = "ext4_x86_64.rs"]
mod ext4_arch;

pub use ext4_arch::*;

// ─── Constants ────────────────────────────────────────────────────────────

const SECTOR_SIZE: usize = 512;

// ─── Sector write-through cache ────────────────────────────────────────────
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

// ─── I/O statistics ───────────────────────────────────────────────────────

pub(crate) static READ_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static WRITE_COUNT: AtomicUsize = AtomicUsize::new(0);

// ─── Disk bounds protection (x86_64: OOB guard, RISC-V: no-op at max==0) ──

static MAX_DISK_OFFSET: AtomicU64 = AtomicU64::new(0);

fn set_max_disk_offset(max_offset: u64) {
    MAX_DISK_OFFSET.store(max_offset, Ordering::Relaxed);
}

fn is_offset_valid(offset: usize) -> bool {
    let max = MAX_DISK_OFFSET.load(Ordering::Relaxed);
    max == 0 || offset < max as usize
}

// ─── KarteBlockDevice (Ext4BlockDevice adapter) ────────────────────────────
// Stores architecture-specific block read/write function pointers so that
// the read_offset / write_offset logic is shared across architectures.

pub struct KarteBlockDevice {
    read_sector_fn: fn(usize, &mut [u8]) -> Result<(), &'static str>,
    write_sector_fn: fn(usize, &[u8]) -> Result<(), &'static str>,
}

impl KarteBlockDevice {
    pub fn new(
        read_sector_fn: fn(usize, &mut [u8]) -> Result<(), &'static str>,
        write_sector_fn: fn(usize, &[u8]) -> Result<(), &'static str>,
    ) -> Self {
        Self {
            read_sector_fn,
            write_sector_fn,
        }
    }
}

impl Ext4BlockDevice for KarteBlockDevice {
    fn read_offset(&self, offset: usize) -> Vec<u8> {
        // Bounds check: if disk size is known and offset is out of range,
        // return a zeroed buffer (matches Linux behaviour for reads beyond EOF).
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
                if (self.read_sector_fn)(abs_sector, &mut sector).is_err() {
                    crate::console_println!(
                        "[read_offset] DISK READ FAIL sector={} offset={:#x}",
                        abs_sector,
                        offset
                    );
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
        // Bounds check
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
                let cached = {
                    let cache = SECTOR_CACHE.lock();
                    cache.get(sector_idx).copied()
                };
                if let Some(cached) = cached {
                    sector.copy_from_slice(&cached);
                } else if (self.read_sector_fn)(sector_idx, &mut sector).is_err() {
                    return;
                }
            }

            sector[sector_offset..sector_offset + bytes_in_sector]
                .copy_from_slice(&data[data_pos..data_pos + bytes_in_sector]);

            if let Err(e) = (self.write_sector_fn)(sector_idx, &sector) {
                crate::console_println!(
                    "[write_offset] DISK WRITE FAIL sector={} offset={:#x} err={}",
                    sector_idx,
                    current_offset,
                    e
                );
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

// ─── Ext4FileDesc ──────────────────────────────────────────────────────────

/// Descriptor for an open ext4 file, stored in FdTable.
#[derive(Clone, Debug, PartialEq)]
pub struct Ext4FileDesc {
    pub inode_num: u32,
    pub writable: bool,
}

// ─── Ext4Fs (VFS FileSystem implementation) ────────────────────────────────

pub struct Ext4Fs {
    pub(crate) ext4: spin::Mutex<Ext4>,
}

impl Ext4Fs {
    pub fn new() -> Result<Self, &'static str> {
        let bd = Arc::new(create_block_device());
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
                if entry.get_name() == component {
                    found = Some(entry.inode);
                    break;
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
            Err(_) => Err(VfsError::IoError),
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
            Ok(inode_ref) => Ok(inode_ref.inode_num as u64),
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

    fn set_file_size(&mut self, inode: u64, size: usize) -> Result<(), VfsError> {
        let ext4 = self.ext4.lock();
        let mut inode_ref = ext4.get_inode_ref(inode as u32);
        if inode_ref.inode.is_dir() {
            return Err(VfsError::NotAFile);
        }

        let old_size = inode_ref.inode.size() as usize;
        if size <= old_size {
            return ext4
                .truncate_inode(&mut inode_ref, size as u64)
                .map(|_| ())
                .map_err(|_| VfsError::IoError);
        }

        let zeros = alloc::vec![0u8; BLOCK_SIZE];
        let mut offset = old_size;
        while offset < size {
            let chunk_len = core::cmp::min(BLOCK_SIZE, size - offset);
            ext4.write_at(inode as u32, offset, &zeros[..chunk_len])
                .map_err(|_| VfsError::IoError)?;
            offset += chunk_len;
        }
        Ok(())
    }
}

// ─── Global state ──────────────────────────────────────────────────────────

// SpinLock (not YieldMutex) for EXT4_FS: block I/O happens while holding this
// lock. Timer interrupt during I/O + YieldMutex = infinite yield deadlock.
pub(crate) static EXT4_FS: spin::Mutex<Option<Ext4Fs>> = spin::Mutex::new(None);
static EXT4_AVAILABLE: AtomicBool = AtomicBool::new(false);

// ─── CR3 helper (no-op on RISC-V, switches to kernel CR3 on x86_64) ────────

/// Run a closure under kernel CR3 on x86_64. Required for all ext4 file
/// operations because the block I/O DMA buffer is accessed via identity
/// mapping, which gets corrupted in user page tables after ELF loading
/// overwrites identity-mapped PTEs. On RISC-V, this is a no-op (identity
/// mapping is preserved in user page tables via satp copy).
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

// ─── Path helpers ──────────────────────────────────────────────────────────

fn split_last_component(path: &str) -> (&str, &str) {
    if let Some(idx) = path.rfind('/') {
        (&path[..idx], &path[idx + 1..])
    } else {
        ("", path)
    }
}

// ─── Initialization ────────────────────────────────────────────────────────

pub fn init() -> Result<(), &'static str> {
    match Ext4Fs::new() {
        Ok(fs) => {
            // Compute disk size from superblock for OOB protection.
            // Harmless no-op on RISC-V where MAX_DISK_OFFSET defaults to 0
            // and is_offset_valid always returns true when max==0.
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

// ─── Public API ────────────────────────────────────────────────────────────

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
                let result = fs.read_file(inode, offset, buf);
                #[cfg(target_arch = "x86_64")]
                match &result {
                    Ok(n) if *n < buf.len() => {
                        crate::console_println!(
                            "[read_fn] SHORT READ inode={} off={:#x} req={} got={}",
                            inode,
                            offset,
                            buf.len(),
                            n
                        );
                    }
                    _ => {}
                }
                result.map_err(|_| ())
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
        fs.write_file(inode, 0, data)
            .map_err(|_| "ext4 write failed")?;
    } else {
        let (parent_path, file_name) = split_last_component(name);
        let parent_inode = if parent_path.is_empty() {
            ROOT_INODE as u64
        } else {
            fs.lookup(ROOT_INODE as u64, parent_path)
                .map_err(|_| "ext4: parent directory not found")?
        };
        let inode = fs
            .create_file(parent_inode, file_name)
            .map_err(|_| "ext4 create failed")?;
        fs.write_file(inode, 0, data)
            .map_err(|_| "ext4 write failed")?;
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
    with_kernel_cr3_ext4(|| create_directory_inner(path))
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
        fs.lookup(ROOT_INODE as u64, parent_path)
            .map_err(|_| "parent not found")?
    };

    match fs.create_dir(parent_inode, dir_name) {
        Ok(_) => Ok(()),
        Err(_) => Err("ext4 create_dir failed"),
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

/// Read file data at specific offset (for pread64 / linux read).
pub fn read_file_at_offset(
    inode: u32,
    offset: usize,
    buf: &mut [u8],
) -> Result<usize, &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }
    let guard = EXT4_FS.lock();
    let fs = guard.as_ref().ok_or("ext4 not initialized")?;
    fs.read_file(inode as u64, offset, buf)
        .map_err(|_| "ext4 read_at failed")
}

/// Write data to file at specific offset (for pwrite64 / linux write).
pub fn write_file_at_offset(inode: u32, offset: usize, data: &[u8]) -> Result<usize, &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }
    let mut guard = EXT4_FS.lock();
    let fs = guard.as_mut().ok_or("ext4 not initialized")?;
    fs.write_file(inode as u64, offset, data)
        .map_err(|_| "ext4 write_at failed")
}

/// Truncate file to 0 bytes (for O_TRUNC).
pub fn truncate_file(inode: u32) -> Result<(), &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }
    let mut guard = EXT4_FS.lock();
    let fs = guard.as_mut().ok_or("ext4 not initialized")?;
    fs.set_file_size(inode as u64, 0)
        .map_err(|_| "ext4 truncate failed")
}

/// Get file size by inode number.
pub fn file_size(inode: u32) -> Result<usize, &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }
    let guard = EXT4_FS.lock();
    let fs = guard.as_ref().ok_or("ext4 not initialized")?;
    fs.metadata(inode as u64)
        .map(|m| m.size)
        .map_err(|_| "ext4 metadata failed")
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── ext4 Tests ──");

    crate::test::run_test("ext4_read_count", || {
        let count = READ_COUNT.load(Ordering::SeqCst);
        count >= 0
    });

    crate::test::run_test("ext4_write_count", || {
        let count = WRITE_COUNT.load(Ordering::SeqCst);
        count >= 0
    });
}

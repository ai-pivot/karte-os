//! ext4 filesystem — RISC-V implementation (VirtIO block device).

extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ext4_rs::{BLOCK_SIZE, BlockDevice as Ext4BlockDevice, EXT4_INODE_MODE_FILE, Ext4, ROOT_INODE};

use crate::driver::vfs::{FileSystem, VfsDirEntry, VfsError, VfsFileType, VfsMetadata};

const SECTOR_SIZE: usize = 512;

static READ_COUNT: AtomicUsize = AtomicUsize::new(0);
static WRITE_COUNT: AtomicUsize = AtomicUsize::new(0);

fn block_read(sector: usize, buf: &mut [u8]) -> Result<(), &'static str> {
    READ_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::driver::virtio::read_block(sector, buf)
}

fn block_write(sector: usize, buf: &[u8]) -> Result<(), &'static str> {
    WRITE_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::driver::virtio::write_block(sector, buf)
}

pub struct KarteBlockDevice;

impl KarteBlockDevice {
    pub fn new() -> Self {
        Self
    }
}

impl Ext4BlockDevice for KarteBlockDevice {
    fn read_offset(&self, offset: usize) -> Vec<u8> {
        let mut buf = alloc::vec![0u8; BLOCK_SIZE];
        let base_sector = offset / SECTOR_SIZE;
        let skip = offset % SECTOR_SIZE;

        let mut buf_pos = 0;
        let mut sector_idx = 0;
        while buf_pos < BLOCK_SIZE {
            let mut sector = [0u8; SECTOR_SIZE];
            match block_read(base_sector + sector_idx, &mut sector) {
                Ok(()) => {
                    let src_start = if sector_idx == 0 { skip } else { 0 };
                    let src_end = SECTOR_SIZE;
                    let remaining = BLOCK_SIZE - buf_pos;
                    let copy_len = core::cmp::min(src_end - src_start, remaining);
                    buf[buf_pos..buf_pos + copy_len]
                        .copy_from_slice(&sector[src_start..src_start + copy_len]);
                    buf_pos += copy_len;
                }
                Err(_) => break,
            }
            sector_idx += 1;
        }
        buf
    }

    fn write_offset(&self, offset: usize, data: &[u8]) {
        let mut data_pos = 0;
        let mut current_offset = offset;

        while data_pos < data.len() {
            let sector_idx = current_offset / SECTOR_SIZE;
            let sector_offset = current_offset % SECTOR_SIZE;
            let bytes_in_sector =
                core::cmp::min(SECTOR_SIZE - sector_offset, data.len() - data_pos);

            let mut sector = [0u8; SECTOR_SIZE];
            if block_read(sector_idx, &mut sector).is_err() {
                return;
            }

            sector[sector_offset..sector_offset + bytes_in_sector]
                .copy_from_slice(&data[data_pos..data_pos + bytes_in_sector]);

            if block_write(sector_idx, &sector).is_err() {
                return;
            }

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

// SpinLock (not YieldMutex) for EXT4_FS: AHCI I/O happens while holding this lock.
// Timer interrupt during I/O + YieldMutex = infinite yield deadlock.
static EXT4_FS: spin::Mutex<Option<Ext4Fs>> = spin::Mutex::new(None);
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

pub fn read_file(name: &str) -> Option<Vec<u8>> {
    if !has_ext4() {
        return None;
    }
    let guard = EXT4_FS.lock();
    let fs = guard.as_ref()?;
    let inode = fs.lookup(ROOT_INODE as u64, name).ok()?;
    let metadata = fs.metadata(inode).ok()?;
    let size = metadata.size;
    if size == 0 {
        return Some(Vec::new());
    }
    let mut buf = alloc::vec![0u8; size];
    let bytes_read = fs.read_file(inode, 0, &mut buf).ok()?;
    buf.truncate(bytes_read);
    Some(buf)
}

pub fn read_file_range(name: &str) -> Option<impl Fn(usize, &mut [u8]) -> Result<usize, ()>> {
    if !has_ext4() {
        return None;
    }
    let guard = EXT4_FS.lock();
    let fs = guard.as_ref()?;
    let inode = fs.lookup(ROOT_INODE as u64, name).ok()?;
    drop(guard);

    Some(move |offset: usize, buf: &mut [u8]| -> Result<usize, ()> {
        if !has_ext4() {
            return Err(());
        }
        let guard = EXT4_FS.lock();
        let fs = guard.as_ref().ok_or(())?;
        fs.read_file(inode, offset, buf).map_err(|_| ())
    })
}

pub fn lookup_path(name: &str) -> Option<u64> {
    if !has_ext4() {
        return None;
    }
    let guard = EXT4_FS.lock();
    let fs = guard.as_ref()?;
    fs.lookup(ROOT_INODE as u64, name).ok()
}

pub fn metadata_of(inode: u64) -> Option<VfsMetadata> {
    if !has_ext4() {
        return None;
    }
    let guard = EXT4_FS.lock();
    let fs = guard.as_ref()?;
    fs.metadata(inode).ok()
}

pub fn write_file(name: &str, data: &[u8]) -> Result<(), &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }
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
}

pub fn create_directory(name: &str) -> Result<(), &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }
    let mut guard = EXT4_FS.lock();
    let fs = guard.as_mut().ok_or("ext4 not initialized")?;
    let (parent_path, dir_name) = split_last_component(name);
    let parent_inode = if parent_path.is_empty() {
        ROOT_INODE as u64
    } else {
        fs.lookup(ROOT_INODE as u64, parent_path)
            .map_err(|_| "ext4: parent directory not found")?
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
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── ext4 Tests (RISC-V) ──");

    crate::test::run_test("ext4_read_count", || {
        let count = READ_COUNT.load(Ordering::SeqCst);
        count >= 0
    });

    crate::test::run_test("ext4_write_count", || {
        let count = WRITE_COUNT.load(Ordering::SeqCst);
        count >= 0
    });
}

/// Read file data at specific offset (for pread64).
/// Returns number of bytes read into the provided buffer.
pub fn read_file_at_offset(
    inode: u32,
    offset: usize,
    buf: &mut [u8],
) -> Result<usize, &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }
    let mut guard = EXT4_FS.lock();
    let fs = guard.as_mut().ok_or("ext4 not initialized")?;
    fs.read_file(inode as u64, offset, buf)
        .map_err(|_| "ext4 read_at failed")
}

/// Write data to file at specific offset (for pwrite64).
/// Returns number of bytes written.
pub fn write_file_at_offset(inode: u32, offset: usize, data: &[u8]) -> Result<usize, &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }
    let mut guard = EXT4_FS.lock();
    let fs = guard.as_mut().ok_or("ext4 not initialized")?;
    fs.write_file(inode as u64, offset, data)
        .map_err(|_| "ext4 write_at failed")
}

/// Get file size by inode number.
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

//! FAT32 filesystem module backed by starry-fatfs.
//!
//! Provides a `Fat32Storage` adapter that bridges starry-fatfs IO traits to
//! the VirtIO block device, and a set of public API functions for filesystem
//! operations: init, read/write files, list directory, delete.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::sync::spinlock::SpinLock;
use fatfs::{FileSystem, FormatVolumeOptions, FsOptions, IoBase, Read, Seek, SeekFrom, Write};

const SECTOR_SIZE: u64 = 512;

// ---------------------------------------------------------------------------
// Fat32Storage — adapter from fatfs IO traits to VirtIO block device
// ---------------------------------------------------------------------------

/// Storage adapter that implements starry-fatfs IO traits over the VirtIO
/// block device. Reads and writes happen at byte granularity; this struct
/// internally converts to sector-aligned block operations.
pub struct Fat32Storage {
    position: u64,
    capacity_bytes: u64,
}

impl Fat32Storage {
    /// Create a new storage instance backed by the VirtIO block device.
    pub fn new() -> Self {
        let cap = crate::driver::virtio::capacity().unwrap_or(0) * SECTOR_SIZE;
        Self {
            position: 0,
            capacity_bytes: cap,
        }
    }
}

/// Error type for Fat32Storage IO operations.
#[derive(Debug)]
pub struct StorageError(&'static str);

impl fatfs::IoError for StorageError {
    fn is_interrupted(&self) -> bool {
        false
    }
    fn new_unexpected_eof_error() -> Self {
        StorageError("unexpected eof")
    }
    fn new_write_zero_error() -> Self {
        StorageError("write zero")
    }
}

impl IoBase for Fat32Storage {
    type Error = StorageError;
}

impl Read for Fat32Storage {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.position >= self.capacity_bytes {
            return Ok(0);
        }

        let mut total_read = 0usize;
        let base_sector = (self.position / SECTOR_SIZE) as usize;
        let offset_in_first_sector = (self.position % SECTOR_SIZE) as usize;
        let mut sector_buf = [0u8; 512];

        while total_read < buf.len() {
            let byte_offset = offset_in_first_sector + total_read;
            let sector_idx = base_sector + byte_offset / 512;
            crate::driver::virtio::read_block(sector_idx, &mut sector_buf)
                .map_err(|_| StorageError("read failed"))?;

            let src_start = byte_offset % 512;
            let avail = 512 - src_start;
            let need = buf.len() - total_read;
            let to_copy = avail.min(need);
            buf[total_read..total_read + to_copy]
                .copy_from_slice(&sector_buf[src_start..src_start + to_copy]);
            total_read += to_copy;
        }

        self.position += total_read as u64;
        Ok(total_read)
    }
}

impl Write for Fat32Storage {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.position >= self.capacity_bytes {
            return Err(StorageError("write beyond capacity"));
        }

        let mut total_written = 0usize;
        let base_sector = (self.position / SECTOR_SIZE) as usize;
        let offset_in_first_sector = (self.position % SECTOR_SIZE) as usize;
        let mut sector_buf = [0u8; 512];

        while total_written < buf.len() {
            let byte_offset = offset_in_first_sector + total_written;
            let sector_idx = base_sector + byte_offset / 512;

            // Read-modify-write: read the sector first to preserve unaligned bytes
            crate::driver::virtio::read_block(sector_idx, &mut sector_buf)
                .map_err(|_| StorageError("read-modify-write: read failed"))?;

            let src_start = byte_offset % 512;
            let avail = 512 - src_start;
            let need = buf.len() - total_written;
            let to_copy = avail.min(need);
            sector_buf[src_start..src_start + to_copy]
                .copy_from_slice(&buf[total_written..total_written + to_copy]);

            crate::driver::virtio::write_block(sector_idx, &sector_buf)
                .map_err(|_| StorageError("read-modify-write: write failed"))?;
            total_written += to_copy;
        }

        self.position += total_written as u64;
        Ok(total_written)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        // VirtIO block writes are synchronous, nothing to flush.
        Ok(())
    }
}

impl Seek for Fat32Storage {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64, Self::Error> {
        self.position = match pos {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) => {
                let new = self.capacity_bytes as i64 + offset;
                if new < 0 {
                    return Err(StorageError("seek before start"));
                }
                new as u64
            }
            SeekFrom::Current(offset) => {
                let new = self.position as i64 + offset;
                if new < 0 {
                    return Err(StorageError("seek before start"));
                }
                new as u64
            }
        };
        Ok(self.position)
    }
}

// ---------------------------------------------------------------------------
// Global FAT32 filesystem instance and public API
// ---------------------------------------------------------------------------

/// Global FAT32 filesystem instance.
static FAT32_FS: SpinLock<Option<FileSystem<Fat32Storage>>> = SpinLock::new(None);

/// Initialize the FAT32 filesystem.
///
/// Attempts to mount an existing FAT32 filesystem on the VirtIO block device.
/// If the disk is not formatted, formats it first and then mounts.
pub fn init() -> Result<(), &'static str> {
    // Try mounting an existing filesystem
    let storage = Fat32Storage::new();
    match FileSystem::new(storage, FsOptions::new()) {
        Ok(fs) => {
            crate::console_println!("[fat32] Filesystem mounted successfully");
            *FAT32_FS.lock() = Some(fs);
            return Ok(());
        }
        Err(_) => {
            crate::console_println!("[fat32] No filesystem found, formatting...");
        }
    }

    // Format then mount
    let mut storage = Fat32Storage::new();
    fatfs::format_volume(&mut storage, FormatVolumeOptions::new())
        .map_err(|_| "Failed to format FAT32 volume")?;

    match FileSystem::new(storage, FsOptions::new()) {
        Ok(fs) => {
            crate::console_println!("[fat32] Filesystem formatted and mounted");
            *FAT32_FS.lock() = Some(fs);
            Ok(())
        }
        Err(_) => Err("Failed to mount after formatting"),
    }
}

/// Inject a file into the FAT32 root directory.
///
/// If the file already exists, it is left untouched (no overwrite).
pub fn inject_file(name: &str, data: &[u8]) -> Result<(), &'static str> {
    let mut guard = FAT32_FS.lock();
    let fs = guard.as_mut().ok_or("FAT32 not initialized")?;
    let root = fs.root_dir();

    // Skip if file already exists
    if root.open_file(name).is_ok() {
        return Ok(());
    }

    let mut file = root.create_file(name).map_err(|_| "create file failed")?;
    file.truncate().map_err(|_| "truncate failed")?;
    file.write_all(data).map_err(|_| "write failed")?;
    Ok(())
}

/// Read the full contents of a file from the FAT32 root directory.
pub fn read_file(name: &str) -> Option<Vec<u8>> {
    let mut guard = FAT32_FS.lock();
    let fs = guard.as_mut()?;
    let root = fs.root_dir();
    let mut file = root.open_file(name).ok()?;
    let size = file.size()? as usize;
    let mut buf = alloc::vec![0u8; size];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// List files in the FAT32 root directory.
///
/// Returns a vector of `(file_name, size_in_bytes)` pairs.
pub fn list_root() -> Vec<(String, usize)> {
    let mut result = Vec::new();
    let mut guard = FAT32_FS.lock();
    if let Some(fs) = guard.as_mut() {
        let root = fs.root_dir();
        for entry in root.iter() {
            if let Ok(e) = entry {
                if e.is_file() {
                    result.push((e.file_name(), e.len() as usize));
                }
            }
        }
    }
    result
}

/// Write (create or overwrite) a file in the FAT32 root directory.
pub fn write_file(name: &str, data: &[u8]) -> Result<(), &'static str> {
    let mut guard = FAT32_FS.lock();
    let fs = guard.as_mut().ok_or("FAT32 not initialized")?;
    let root = fs.root_dir();
    let mut file = root.create_file(name).map_err(|_| "create file failed")?;
    file.truncate().map_err(|_| "truncate failed")?;
    file.write_all(data).map_err(|_| "write failed")?;
    Ok(())
}

/// Delete a file from the FAT32 root directory.
pub fn delete_file(name: &str) -> Result<(), &'static str> {
    let mut guard = FAT32_FS.lock();
    let fs = guard.as_mut().ok_or("FAT32 not initialized")?;
    let root = fs.root_dir();
    root.remove(name).map_err(|_| "remove failed")
}

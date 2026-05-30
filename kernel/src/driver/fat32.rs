//! FAT32 filesystem module backed by starry-fatfs.
//!
//! Only available on RISC-V (requires VirtIO block device).
//! On x86_64, provides stub functions that return "not available".

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

static FAT32_AVAILABLE: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "riscv64")]
mod riscv_impl {
    extern crate alloc;

    use alloc::string::String;
    use alloc::vec::Vec;

    use crate::sync::spinlock::SpinLock;
    use fatfs::{FileSystem, FormatVolumeOptions, FsOptions, IoBase, Read, Seek, SeekFrom, Write};

    const SECTOR_SIZE: u64 = 512;

    pub struct Fat32Storage {
        position: u64,
        capacity_bytes: u64,
    }

    impl Fat32Storage {
        pub fn new() -> Self {
            let cap = crate::driver::virtio::capacity().unwrap_or(0) * SECTOR_SIZE;
            Self {
                position: 0,
                capacity_bytes: cap,
            }
        }
    }

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

    static FAT32_FS: SpinLock<Option<FileSystem<Fat32Storage>>> = SpinLock::new(None);

    pub fn init() -> Result<(), &'static str> {
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

        let mut storage = Fat32Storage::new();
        fatfs::format_volume(
            &mut storage,
            FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat32),
        )
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

    pub fn inject_file(name: &str, data: &[u8]) -> Result<(), &'static str> {
        let mut guard = FAT32_FS.lock();
        let fs = guard.as_mut().ok_or("FAT32 not initialized")?;
        let root = fs.root_dir();
        if root.open_file(name).is_ok() {
            return Ok(());
        }
        let mut file = root.create_file(name).map_err(|_| "create failed")?;
        file.write_all(data).map_err(|_| "write failed")?;
        Ok(())
    }

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

    pub fn write_file(name: &str, data: &[u8]) -> Result<(), &'static str> {
        let mut guard = FAT32_FS.lock();
        let fs = guard.as_mut().ok_or("FAT32 not initialized")?;
        let root = fs.root_dir();
        let mut file = root.create_file(name).map_err(|_| "create failed")?;
        file.write_all(data).map_err(|_| "write failed")?;
        Ok(())
    }

    pub fn list_root() -> Vec<(String, usize)> {
        let mut result = Vec::new();
        let mut guard = FAT32_FS.lock();
        let Some(fs) = guard.as_mut() else {
            return result;
        };
        let root = fs.root_dir();
        let mut iter = root.iter();
        while let Some(Ok(entry)) = iter.next() {
            let name = entry.file_name();
            let len = entry.len();
            result.push((name, len as usize));
        }
        result
    }

    pub fn remove_file(name: &str) -> Result<(), &'static str> {
        let mut guard = FAT32_FS.lock();
        let fs = guard.as_mut().ok_or("FAT32 not initialized")?;
        let root = fs.root_dir();
        root.remove(name).map_err(|_| "remove failed")
    }
}

#[cfg(target_arch = "x86_64")]
mod x86_64_stub {
    use alloc::string::String;
    use alloc::vec::Vec;

    pub fn init() -> Result<(), &'static str> {
        Err("FAT32 not available on x86_64 (no block device)")
    }

    pub fn inject_file(_name: &str, _data: &[u8]) -> Result<(), &'static str> {
        Err("FAT32 not available on x86_64")
    }

    pub fn read_file(_name: &str) -> Option<Vec<u8>> {
        None
    }

    pub fn write_file(_name: &str, _data: &[u8]) -> Result<(), &'static str> {
        Err("FAT32 not available on x86_64")
    }

    pub fn list_root() -> Vec<(String, usize)> {
        Vec::new()
    }

    pub fn remove_file(_name: &str) -> Result<(), &'static str> {
        Err("FAT32 not available on x86_64")
    }
}

#[cfg(target_arch = "riscv64")]
pub use riscv_impl::*;

#[cfg(target_arch = "x86_64")]
pub use x86_64_stub::*;

pub fn has_fat32() -> bool {
    FAT32_AVAILABLE.load(Ordering::Relaxed)
}

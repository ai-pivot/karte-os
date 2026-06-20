//! ext4 filesystem — x86_64 block I/O callbacks (AHCI or VirtIO dispatch).

extern crate alloc;

use alloc::boxed::Box;
use core::sync::atomic::Ordering;

use ext4_rs::ROOT_INODE;

use super::KarteBlockDevice;

// ─── Block I/O dispatch ────────────────────────────────────────────────────
// AHCI first, VirtIO fallback.

fn x86_block_read(sector: usize, buf: &mut [u8]) -> Result<(), &'static str> {
    super::READ_COUNT.fetch_add(1, Ordering::Relaxed);
    if crate::driver::ahci::is_available() {
        crate::driver::ahci::read_block(sector, buf)
    } else {
        crate::arch::virtio_blk::read_block(sector, buf)
    }
}

fn x86_block_write(sector: usize, buf: &[u8]) -> Result<(), &'static str> {
    super::WRITE_COUNT.fetch_add(1, Ordering::Relaxed);
    if crate::driver::ahci::is_available() {
        crate::driver::ahci::write_block(sector, buf)
    } else {
        crate::arch::virtio_blk::write_block(sector, buf)
    }
}

/// Create a KarteBlockDevice backed by AHCI/VirtIO block I/O.
/// Called from `ext4.rs::init()`.
pub fn create_block_device() -> KarteBlockDevice {
    KarteBlockDevice::new(x86_block_read, x86_block_write)
}

// ─── VFS Integration (CR3-switching wrapper) ────────────────────────────────
// On x86_64, all ext4 file operations must run under kernel CR3 because
// the block I/O DMA buffer is accessed via identity mapping, which gets
// corrupted in user page tables after ELF loading.

/// Wrapper that implements the VFS FileSystem trait for ext4.
/// All operations are routed through `super::EXT4_FS` under kernel CR3.
pub struct Ext4FileSystem;

impl crate::driver::vfs::FileSystem for Ext4FileSystem {
    fn name(&self) -> &str {
        "ext4"
    }

    fn root_inode(&self) -> u64 {
        ROOT_INODE as u64
    }

    fn lookup(&self, dir: u64, name: &str) -> Result<u64, crate::driver::vfs::VfsError> {
        super::with_kernel_cr3_ext4(|| {
            let guard = super::EXT4_FS.lock();
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
        super::with_kernel_cr3_ext4(|| {
            let guard = super::EXT4_FS.lock();
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
        super::with_kernel_cr3_ext4(|| {
            let guard = super::EXT4_FS.lock();
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
        super::with_kernel_cr3_ext4(|| {
            let guard = super::EXT4_FS.lock();
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
        super::with_kernel_cr3_ext4(|| {
            let mut guard = super::EXT4_FS.lock();
            let fs = guard
                .as_mut()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            let result = fs
                .write_file(inode, offset, data)
                .map_err(|_| crate::driver::vfs::VfsError::IoError)?;

            // Update inode mtime/ctime after successful write
            let ext4 = fs.ext4.lock();
            let mut inode_ref = ext4.get_inode_ref(inode as u32);
            let (sec, _) = crate::arch::rtc::wall_clock();
            inode_ref.inode.set_mtime(sec as u32);
            inode_ref.inode.set_ctime(sec as u32);
            ext4.write_back_inode(&mut inode_ref);
            drop(ext4);

            Ok(result)
        })
    }

    fn create_file(&mut self, dir: u64, name: &str) -> Result<u64, crate::driver::vfs::VfsError> {
        super::with_kernel_cr3_ext4(|| {
            let mut guard = super::EXT4_FS.lock();
            let fs = guard
                .as_mut()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            let ext4 = fs.ext4.lock();
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
        super::with_kernel_cr3_ext4(|| {
            let mut guard = super::EXT4_FS.lock();
            let fs = guard
                .as_mut()
                .ok_or(crate::driver::vfs::VfsError::NotFound)?;
            let ext4 = fs.ext4.lock();
            let inode_ref = ext4
                .create(dir as u32, name, 0o40755 as u16)
                .map_err(|_| crate::driver::vfs::VfsError::IoError)?;
            Ok(inode_ref.inode_num as u64)
        })
    }

    fn unlink(&mut self, dir: u64, name: &str) -> Result<(), crate::driver::vfs::VfsError> {
        super::with_kernel_cr3_ext4(|| {
            let mut guard = super::EXT4_FS.lock();
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
        super::with_kernel_cr3_ext4(|| {
            let mut guard = super::EXT4_FS.lock();
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
    crate::driver::vfs::mount("/", Box::new(Ext4FileSystem))
        .map_err(|_| "failed to mount ext4 to VFS")
}

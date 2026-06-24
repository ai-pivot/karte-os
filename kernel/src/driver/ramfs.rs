// kernel/src/driver/ramfs.rs — RAM-backed filesystem implementing vfs::FileSystem

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::driver::vfs::{FileSystem, VfsDirEntry, VfsError, VfsFileType, VfsMetadata};
use crate::sync::spinlock::SpinLock;

// ─── RamFS File Node ──────────────────────────────────────────────────────

/// File data: either a static reference (for embedded binaries) or heap-allocated.
#[derive(Clone)]
enum RamFileData {
    Static(&'static [u8]),
    Owned(Vec<u8>),
}

impl RamFileData {
    fn as_slice(&self) -> &[u8] {
        match self {
            RamFileData::Static(s) => s,
            RamFileData::Owned(v) => v,
        }
    }

    fn len(&self) -> usize {
        self.as_slice().len()
    }
}

/// A file or directory node in RamFS
struct RamFsNode {
    name: String,
    data: RamFileData,
    is_dir: bool,
    /// Children inodes (for directories)
    children: Vec<u64>,
}

// ─── RamFS Implementation ─────────────────────────────────────────────────

pub struct RamFileSystem {
    nodes: BTreeMap<u64, RamFsNode>,
    next_inode: u64,
}

/// Root directory inode
const ROOT_INODE: u64 = 1;

impl RamFileSystem {
    /// Create a new empty RamFS with a root directory
    pub fn new() -> Self {
        let mut fs = Self {
            nodes: BTreeMap::new(),
            next_inode: 2, // inode 1 is root
        };
        // Create root directory node
        fs.nodes.insert(
            ROOT_INODE,
            RamFsNode {
                name: String::from("/"),
                data: RamFileData::Owned(Vec::new()),
                is_dir: true,
                children: Vec::new(),
            },
        );
        fs
    }

    /// Write static data to a file (no heap allocation for data).
    /// Creates the file in the root directory if it doesn't exist.
    pub fn write_static(&mut self, name: &str, data: &'static [u8]) -> Result<(), VfsError> {
        // Find existing file inode by name in root directory
        let existing_inode = self.find_child_by_name(ROOT_INODE, name);
        if let Some(inode) = existing_inode {
            if let Some(node) = self.nodes.get_mut(&inode) {
                node.data = RamFileData::Static(data);
                return Ok(());
            }
        }

        // Create new file in root directory
        let inode = self.alloc_inode();
        let node = RamFsNode {
            name: String::from(name),
            data: RamFileData::Static(data),
            is_dir: false,
            children: Vec::new(),
        };
        self.nodes.insert(inode, node);
        self.add_child(ROOT_INODE, inode);
        Ok(())
    }

    /// Create a new RamFS pre-populated with embedded user programs.
    pub fn new_initialized() -> Self {
        let mut fs = Self::new();

        // Assembly test programs are RISC-V only
        #[cfg(target_arch = "riscv64")]
        {
            fs.write_static("hello", include_bytes!("../../../user/hello.elf"))
                .unwrap();
            fs.write_static("heap_test", include_bytes!("../../../user/heap_test.elf"))
                .unwrap();
            fs.write_static("file_test", include_bytes!("../../../user/file_test.elf"))
                .unwrap();
            fs.write_static("spawn_test", include_bytes!("../../../user/spawn_test.elf"))
                .unwrap();
        }

        fs.write_static("shell", include_bytes!("../../../user/shell.elf"))
            .unwrap();
        fs.write_static("init", include_bytes!("../../../user/shell.elf"))
            .unwrap();

        // xbot-cli is NOT embedded — 69MB binary makes EFI loader too large
        // for real UEFI firmware. Load from ext4 disk instead.
        // fs.write_static("xbot-cli", include_bytes!("../../../user/xbot-cli"))
        //     .unwrap();

        fs
    }

    /// Allocate a new inode number
    fn alloc_inode(&mut self) -> u64 {
        let inode = self.next_inode;
        self.next_inode += 1;
        inode
    }

    /// Add a child inode to a directory node
    fn add_child(&mut self, dir_inode: u64, child_inode: u64) {
        if let Some(dir) = self.nodes.get_mut(&dir_inode) {
            if !dir.children.contains(&child_inode) {
                dir.children.push(child_inode);
            }
        }
    }

    /// Find a child inode by name in a directory
    fn find_child_by_name(&self, dir_inode: u64, name: &str) -> Option<u64> {
        let dir = self.nodes.get(&dir_inode)?;
        if !dir.is_dir {
            return None;
        }
        for &child_inode in &dir.children {
            if let Some(child) = self.nodes.get(&child_inode) {
                if child.name == name {
                    return Some(child_inode);
                }
            }
        }
        None
    }

    /// Remove a child inode from a directory node
    fn remove_child(&mut self, dir_inode: u64, child_inode: u64) {
        if let Some(dir) = self.nodes.get_mut(&dir_inode) {
            dir.children.retain(|&c| c != child_inode);
        }
    }

    /// Get the number of user files (excluding root directory)
    pub fn file_count(&self) -> usize {
        self.nodes.len() - 1
    }
}

impl FileSystem for RamFileSystem {
    fn name(&self) -> &str {
        "ramfs"
    }

    fn root_inode(&self) -> u64 {
        ROOT_INODE
    }

    fn lookup(&self, dir: u64, name: &str) -> Result<u64, VfsError> {
        self.find_child_by_name(dir, name).ok_or(VfsError::NotFound)
    }

    fn metadata(&self, inode: u64) -> Result<VfsMetadata, VfsError> {
        let node = self.nodes.get(&inode).ok_or(VfsError::NotFound)?;
        Ok(VfsMetadata {
            file_type: if node.is_dir {
                VfsFileType::Directory
            } else {
                VfsFileType::File
            },
            size: node.data.len(),
            name: node.name.clone(),
        })
    }

    fn readdir(&self, dir: u64, idx: usize) -> Result<Option<VfsDirEntry>, VfsError> {
        let node = self.nodes.get(&dir).ok_or(VfsError::NotFound)?;
        if !node.is_dir {
            return Err(VfsError::NotADirectory);
        }
        if idx >= node.children.len() {
            return Ok(None);
        }
        let child_inode = node.children[idx];
        let child = self.nodes.get(&child_inode).ok_or(VfsError::NotFound)?;
        Ok(Some(VfsDirEntry {
            name: child.name.clone(),
            file_type: if child.is_dir {
                VfsFileType::Directory
            } else {
                VfsFileType::File
            },
            size: child.data.len(),
        }))
    }

    fn read_file(&self, inode: u64, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let node = self.nodes.get(&inode).ok_or(VfsError::NotFound)?;
        if node.is_dir {
            return Err(VfsError::NotAFile);
        }
        let data = node.data.as_slice();
        if offset >= data.len() {
            return Ok(0);
        }
        let end = core::cmp::min(offset + buf.len(), data.len());
        let bytes_to_copy = end - offset;
        buf[..bytes_to_copy].copy_from_slice(&data[offset..end]);
        Ok(bytes_to_copy)
    }

    fn write_file(&mut self, inode: u64, offset: usize, data: &[u8]) -> Result<usize, VfsError> {
        let node = self.nodes.get_mut(&inode).ok_or(VfsError::NotFound)?;
        if node.is_dir {
            return Err(VfsError::NotAFile);
        }
        // Convert to Owned if currently Static
        let mut owned = match &node.data {
            RamFileData::Static(s) => s.to_vec(),
            RamFileData::Owned(v) => v.clone(),
        };
        // Extend if needed
        if offset + data.len() > owned.len() {
            owned.resize(offset + data.len(), 0);
        }
        owned[offset..offset + data.len()].copy_from_slice(data);
        node.data = RamFileData::Owned(owned);
        Ok(data.len())
    }

    fn create_file(&mut self, dir: u64, name: &str) -> Result<u64, VfsError> {
        // Check parent is a directory
        let parent = self.nodes.get(&dir).ok_or(VfsError::NotFound)?;
        if !parent.is_dir {
            return Err(VfsError::NotADirectory);
        }
        // Check no existing entry with same name
        if self.find_child_by_name(dir, name).is_some() {
            return Err(VfsError::AlreadyExists);
        }
        let inode = self.alloc_inode();
        let node = RamFsNode {
            name: String::from(name),
            data: RamFileData::Owned(Vec::new()),
            is_dir: false,
            children: Vec::new(),
        };
        self.nodes.insert(inode, node);
        self.add_child(dir, inode);
        Ok(inode)
    }

    fn create_dir(&mut self, dir: u64, name: &str) -> Result<u64, VfsError> {
        // Check parent is a directory
        let parent = self.nodes.get(&dir).ok_or(VfsError::NotFound)?;
        if !parent.is_dir {
            return Err(VfsError::NotADirectory);
        }
        // Check no existing entry with same name
        if self.find_child_by_name(dir, name).is_some() {
            return Err(VfsError::AlreadyExists);
        }
        let inode = self.alloc_inode();
        let node = RamFsNode {
            name: String::from(name),
            data: RamFileData::Owned(Vec::new()),
            is_dir: true,
            children: Vec::new(),
        };
        self.nodes.insert(inode, node);
        self.add_child(dir, inode);
        Ok(inode)
    }

    fn unlink(&mut self, dir: u64, name: &str) -> Result<(), VfsError> {
        let inode = self
            .find_child_by_name(dir, name)
            .ok_or(VfsError::NotFound)?;
        // Check directory is empty before removing
        let node = self.nodes.get(&inode).ok_or(VfsError::NotFound)?;
        if node.is_dir && !node.children.is_empty() {
            return Err(VfsError::DirectoryNotEmpty);
        }
        self.remove_child(dir, inode);
        self.nodes.remove(&inode);
        Ok(())
    }

    fn set_file_size(&mut self, inode: u64, size: usize) -> Result<(), VfsError> {
        let node = self.nodes.get_mut(&inode).ok_or(VfsError::NotFound)?;
        if node.is_dir {
            return Err(VfsError::NotAFile);
        }
        let mut owned = match &node.data {
            RamFileData::Static(s) => s.to_vec(),
            RamFileData::Owned(v) => v.clone(),
        };
        owned.resize(size, 0);
        node.data = RamFileData::Owned(owned);
        Ok(())
    }
}

// ─── Path-based helpers (for virtual filesystem API) ─────────────────

impl RamFileSystem {
    /// Walk a multi-level path like "/proc/version", creating intermediate
    /// directories as needed. Returns the target parent inode and the final
    /// component name, or an error.
    fn walk_create_dirs(&mut self, path: &str) -> Result<(u64, String), VfsError> {
        let trimmed = path.strip_prefix('/').unwrap_or(path);
        let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Err(VfsError::InvalidParam);
        }
        let mut dir_inode = ROOT_INODE;
        // All but the last component are directories to create/find
        for i in 0..parts.len() - 1 {
            let part = parts[i];
            match self.find_child_by_name(dir_inode, part) {
                Some(child) => {
                    let child_node = self.nodes.get(&child).ok_or(VfsError::NotFound)?;
                    if !child_node.is_dir {
                        return Err(VfsError::NotADirectory);
                    }
                    dir_inode = child;
                }
                None => {
                    // Create intermediate directory
                    dir_inode = self.create_dir(dir_inode, part)?;
                }
            }
        }
        let file_name = String::from(*parts.last().unwrap());
        Ok((dir_inode, file_name))
    }

    /// Walk a path to find the inode of a file or directory. Does NOT create.
    fn walk_path(&self, path: &str) -> Result<u64, VfsError> {
        let trimmed = path.strip_prefix('/').unwrap_or(path);
        if trimmed.is_empty() {
            return Ok(ROOT_INODE);
        }
        let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
        let mut inode = ROOT_INODE;
        for part in parts {
            inode = self
                .find_child_by_name(inode, part)
                .ok_or(VfsError::NotFound)?;
        }
        Ok(inode)
    }

    /// Create (or overwrite) a file at the given path with the given data.
    /// Intermediate directories are created as needed.
    pub fn create_path(&mut self, path: &str, data: &[u8]) -> Result<(), VfsError> {
        let (dir_inode, file_name) = self.walk_create_dirs(path)?;
        // Check if file exists
        if let Some(existing) = self.find_child_by_name(dir_inode, &file_name) {
            // Overwrite
            if let Some(node) = self.nodes.get_mut(&existing) {
                node.data = RamFileData::Owned(data.to_vec());
                return Ok(());
            }
        }
        // Create new file
        let inode = self.alloc_inode();
        let node = RamFsNode {
            name: file_name,
            data: RamFileData::Owned(data.to_vec()),
            is_dir: false,
            children: Vec::new(),
        };
        self.nodes.insert(inode, node);
        self.add_child(dir_inode, inode);
        Ok(())
    }

    /// Read file data at path into an owned Vec.
    pub fn read_path(&self, path: &str) -> Option<Vec<u8>> {
        let inode = self.walk_path(path).ok()?;
        let node = self.nodes.get(&inode)?;
        if node.is_dir {
            return None;
        }
        Some(node.data.as_slice().to_vec())
    }

    /// Check if a path exists (file or directory).
    pub fn path_exists(&self, path: &str) -> bool {
        self.walk_path(path).is_ok()
    }

    /// List children of a directory path. Returns Vec of (name, is_dir, size).
    pub fn readdir_path(&self, path: &str) -> Option<Vec<(String, bool, usize)>> {
        let inode = self.walk_path(path).ok()?;
        let dir_node = self.nodes.get(&inode)?;
        if !dir_node.is_dir {
            return None;
        }
        let mut entries = Vec::new();
        for &child_inode in &dir_node.children {
            if let Some(child) = self.nodes.get(&child_inode) {
                entries.push((child.name.clone(), child.is_dir, child.data.len()));
            }
        }
        Some(entries)
    }

    /// Get file size at path. Returns None if not found or is a directory.
    pub fn file_size(&self, path: &str) -> Option<usize> {
        let inode = self.walk_path(path).ok()?;
        let node = self.nodes.get(&inode)?;
        if node.is_dir {
            return None;
        }
        Some(node.data.len())
    }

    /// Write data to an existing file at path. Returns bytes written.
    pub fn write_path(
        &mut self,
        path: &str,
        offset: usize,
        data: &[u8],
    ) -> Result<usize, VfsError> {
        let inode = self.walk_path(path)?;
        self.write_file(inode, offset, data)
    }
}

// ─── Global Virtual RamFS ─────────────────────────────────────────────────

/// Global RamFS instance for virtual files (/proc, /sys, /etc, etc.)
/// Uses Option to allow non-const initialization at runtime.
static VIRTUAL_RAMFS: SpinLock<Option<RamFileSystem>> = SpinLock::new(None);

/// Ensure the virtual RamFS is initialized (called lazily on first access).
fn ensure_virtual_ramfs() {
    let mut guard = VIRTUAL_RAMFS.lock();
    if guard.is_none() {
        *guard = Some(RamFileSystem::new());
    }
}

/// Initialize the virtual RamFS and pre-populate with standard files.
pub fn virtual_init() {
    ensure_virtual_ramfs();
    let mut vfs = VIRTUAL_RAMFS.lock();
    let ramfs = vfs.as_mut().unwrap();
    // /proc
    let _ = ramfs.create_path("/proc/version", b"Linux 6.1.0 KarteOS x86_64");
    // /etc
    let _ = ramfs.create_path("/etc/resolv.conf", b"nameserver 10.0.2.3");
    let _ = ramfs.create_path("/etc/hostname", b"karteos");
    let _ = ramfs.create_path("/etc/passwd", b"root:x:0:0:root:/root:/bin/sh");
    let _ = ramfs.create_path("/etc/group", b"root:x:0:");
}

/// Create (or overwrite) a virtual file at path.
pub fn virtual_create(path: &str, data: &[u8]) {
    ensure_virtual_ramfs();
    let mut vfs = VIRTUAL_RAMFS.lock();
    let _ = vfs.as_mut().unwrap().create_path(path, data);
}

/// Read virtual file at path.
pub fn virtual_read(path: &str) -> Option<Vec<u8>> {
    ensure_virtual_ramfs();
    let vfs = VIRTUAL_RAMFS.lock();
    vfs.as_ref().unwrap().read_path(path)
}

/// Check if a virtual path exists.
pub fn virtual_exists(path: &str) -> bool {
    ensure_virtual_ramfs();
    let vfs = VIRTUAL_RAMFS.lock();
    vfs.as_ref().unwrap().path_exists(path)
}

/// List a virtual directory. Returns Vec of (name, is_dir, size).
pub fn virtual_readdir(path: &str) -> Option<Vec<(String, bool, usize)>> {
    ensure_virtual_ramfs();
    let vfs = VIRTUAL_RAMFS.lock();
    vfs.as_ref().unwrap().readdir_path(path)
}

/// Get virtual file size.
pub fn virtual_size(path: &str) -> Option<usize> {
    ensure_virtual_ramfs();
    let vfs = VIRTUAL_RAMFS.lock();
    vfs.as_ref().unwrap().file_size(path)
}

/// Write to an existing virtual file.
pub fn virtual_write(path: &str, offset: usize, data: &[u8]) -> Result<usize, VfsError> {
    ensure_virtual_ramfs();
    let mut vfs = VIRTUAL_RAMFS.lock();
    vfs.as_mut().unwrap().write_path(path, offset, data)
}

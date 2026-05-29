// kernel/src/driver/vfs.rs — Virtual File System abstraction layer

extern crate alloc;

use crate::sync::spinlock::SpinLock;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

// ─── Error Types ──────────────────────────────────────────────────────────

/// VFS error type
#[derive(Debug, Clone)]
pub enum VfsError {
    NotFound,
    AlreadyExists,
    NotADirectory,
    NotAFile,
    PermissionDenied,
    IoError,
    InvalidParam,
    OutOfMemory,
    DirectoryNotEmpty,
}

// ─── File Types & Metadata ────────────────────────────────────────────────

/// File type: regular file or directory
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VfsFileType {
    File,
    Directory,
}

/// File/directory metadata
#[derive(Debug, Clone)]
pub struct VfsMetadata {
    pub file_type: VfsFileType,
    pub size: usize,
    pub name: String,
}

impl VfsMetadata {
    pub fn is_dir(&self) -> bool {
        self.file_type == VfsFileType::Directory
    }
}

/// Directory entry (returned by readdir)
#[derive(Debug, Clone)]
pub struct VfsDirEntry {
    pub name: String,
    pub file_type: VfsFileType,
    pub size: usize,
}

// ─── File Open Flags ──────────────────────────────────────────────────────

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0x100;
pub const O_TRUNC: u32 = 0x200;

// ─── FileSystem Trait ────────────────────────────────────────────────────

/// File system trait — FAT32, RamFS, ext4 all implement it.
///
/// inode numbers are defined by each concrete filesystem.
/// RamFS uses a simple counter, FAT32 can use cluster numbers, etc.
pub trait FileSystem: Send + Sync {
    /// File system name
    fn name(&self) -> &str;

    /// Get the root directory's inode number
    fn root_inode(&self) -> u64;

    /// Look up an entry named `name` in directory `dir`, return its inode
    fn lookup(&self, dir: u64, name: &str) -> Result<u64, VfsError>;

    /// Get metadata for inode
    fn metadata(&self, inode: u64) -> Result<VfsMetadata, VfsError>;

    /// Read the `idx`-th directory entry from directory `dir` (None = done)
    fn readdir(&self, dir: u64, idx: usize) -> Result<Option<VfsDirEntry>, VfsError>;

    /// Read from file `inode` at `offset` into `buf`, return bytes read
    fn read_file(&self, inode: u64, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError>;

    /// Write `data` to file `inode` at `offset`, return bytes written
    fn write_file(&mut self, inode: u64, offset: usize, data: &[u8]) -> Result<usize, VfsError>;

    /// Create a new file named `name` in directory `dir`, return its inode
    fn create_file(&mut self, dir: u64, name: &str) -> Result<u64, VfsError>;

    /// Create a new subdirectory named `name` in directory `dir`, return its inode
    fn create_dir(&mut self, dir: u64, name: &str) -> Result<u64, VfsError>;

    /// Remove the entry named `name` from directory `dir`
    fn unlink(&mut self, dir: u64, name: &str) -> Result<(), VfsError>;

    /// Set file size (truncate or extend)
    fn set_file_size(&mut self, inode: u64, size: usize) -> Result<(), VfsError>;
}

// ─── Open File ────────────────────────────────────────────────────────────

/// Kernel-global open file descriptor
#[derive(Clone)]
pub struct OpenFile {
    pub mount_id: usize,
    pub inode: u64,
    pub pos: usize,
    pub flags: u32, // O_RDONLY=0, O_WRONLY=1, O_RDWR=2
}

// ─── Open File Table ──────────────────────────────────────────────────────

/// Global open file table
pub struct OpenFileTable {
    files: Vec<Option<OpenFile>>,
}

impl OpenFileTable {
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// Allocate a new fd, returns the fd number
    pub fn alloc(&mut self, mount_id: usize, inode: u64, flags: u32) -> Result<usize, VfsError> {
        // Reuse a freed slot first
        for (i, slot) in self.files.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(OpenFile {
                    mount_id,
                    inode,
                    pos: 0,
                    flags,
                });
                return Ok(i);
            }
        }
        // No free slot, append new (max 64 open files)
        if self.files.len() >= 64 {
            return Err(VfsError::OutOfMemory);
        }
        let fd = self.files.len();
        self.files.push(Some(OpenFile {
            mount_id,
            inode,
            pos: 0,
            flags,
        }));
        Ok(fd)
    }

    /// Get an immutable reference to an open file
    pub fn get(&self, fd: usize) -> Option<&OpenFile> {
        self.files.get(fd).and_then(|opt| opt.as_ref())
    }

    /// Get a mutable reference to an open file
    pub fn get_mut(&mut self, fd: usize) -> Option<&mut OpenFile> {
        self.files.get_mut(fd).and_then(|opt| opt.as_mut())
    }

    /// Close an open file, returns true if it was open
    pub fn close(&mut self, fd: usize) -> bool {
        if let Some(slot) = self.files.get_mut(fd) {
            if slot.is_some() {
                *slot = None;
                return true;
            }
        }
        false
    }
}

// ─── Mount Table ──────────────────────────────────────────────────────────

/// Maximum number of mounted filesystems
const MAX_MOUNTS: usize = 8;

/// A mount entry binding a filesystem to a path prefix
struct MountEntry {
    prefix: String,
    fs: Box<dyn FileSystem>,
}

// ─── VFS Global State ──────────────────────────────────────────────────────

pub struct VfsState {
    mounts: Vec<MountEntry>,
    open_files: OpenFileTable,
}

static VFS: SpinLock<VfsState> = SpinLock::new(VfsState::new_internal());

impl VfsState {
    const fn new_internal() -> Self {
        Self {
            mounts: Vec::new(),
            open_files: OpenFileTable { files: Vec::new() },
        }
    }
}

// ─── Public VFS API ────────────────────────────────────────────────────────

/// Mount a filesystem at the given path prefix.
pub fn mount(prefix: &str, fs: Box<dyn FileSystem>) -> Result<(), VfsError> {
    let mut vfs = VFS.lock();
    if vfs.mounts.len() >= MAX_MOUNTS {
        return Err(VfsError::OutOfMemory);
    }
    vfs.mounts.push(MountEntry {
        prefix: String::from(prefix),
        fs,
    });
    Ok(())
}

/// Open a file by path, return fd
pub fn open(path: &str, flags: u32) -> Result<usize, VfsError> {
    let mut vfs = VFS.lock();
    let (mount_id, relative_path) = resolve_path_locked(&vfs, path)?;

    // Determine inode to open
    let root_inode = {
        let mount = vfs.mounts.get(mount_id).ok_or(VfsError::NotFound)?;
        mount.fs.root_inode()
    };

    let inode = if relative_path.is_empty() || relative_path == "/" {
        root_inode
    } else {
        let mount = vfs.mounts.get(mount_id).ok_or(VfsError::NotFound)?;
        walk_path(&*mount.fs, &relative_path)?
    };

    // If O_CREAT and path resolved to root, try creating the file
    if flags & O_CREAT != 0 && inode == root_inode {
        // Check if path is just "/" — can't create that
        if relative_path.is_empty() || relative_path == "/" {
            return Err(VfsError::AlreadyExists);
        }
        // Create in parent directory
        let (parent_path, file_name) = split_filename(&relative_path)?;
        let parent_inode = {
            let mount = vfs.mounts.get(mount_id).ok_or(VfsError::NotFound)?;
            walk_path(&*mount.fs, parent_path)?
        };
        let mount = vfs.mounts.get_mut(mount_id).ok_or(VfsError::NotFound)?;
        let inode = mount.fs.create_file(parent_inode, file_name)?;
        vfs.open_files.alloc(mount_id, inode, flags)
    } else {
        // If O_TRUNC, truncate the file
        if flags & O_TRUNC != 0 {
            let mount = vfs.mounts.get_mut(mount_id).ok_or(VfsError::NotFound)?;
            mount.fs.set_file_size(inode, 0)?;
        }
        vfs.open_files.alloc(mount_id, inode, flags)
    }
}

/// Read from fd into buf, return bytes read
pub fn read(fd: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
    let mut vfs = VFS.lock();
    let of = vfs.open_files.get_mut(fd).ok_or(VfsError::InvalidParam)?;
    if of.flags == O_WRONLY {
        return Err(VfsError::PermissionDenied);
    }
    let mount_id = of.mount_id;
    let inode = of.inode;
    let offset = of.pos;
    let mount = vfs.mounts.get(mount_id).ok_or(VfsError::NotFound)?;
    let bytes_read = mount.fs.read_file(inode, offset, buf)?;
    let of = vfs.open_files.get_mut(fd).ok_or(VfsError::InvalidParam)?;
    of.pos += bytes_read;
    Ok(bytes_read)
}

/// Write data to fd, return bytes written
pub fn write(fd: usize, data: &[u8]) -> Result<usize, VfsError> {
    let mut vfs = VFS.lock();
    let of = vfs.open_files.get_mut(fd).ok_or(VfsError::InvalidParam)?;
    if of.flags == O_RDONLY {
        return Err(VfsError::PermissionDenied);
    }
    let mount_id = of.mount_id;
    let inode = of.inode;
    let offset = of.pos;
    let mount = vfs.mounts.get_mut(mount_id).ok_or(VfsError::NotFound)?;
    let bytes_written = mount.fs.write_file(inode, offset, data)?;
    let of = vfs.open_files.get_mut(fd).ok_or(VfsError::InvalidParam)?;
    of.pos += bytes_written;
    Ok(bytes_written)
}

/// Close an open file
pub fn close(fd: usize) -> bool {
    let mut vfs = VFS.lock();
    vfs.open_files.close(fd)
}

/// List directory contents at path, write entries into buf (comma-separated), return total bytes written
pub fn ls(path: &str, buf: &mut [u8], buf_len: usize) -> Result<usize, VfsError> {
    let vfs = VFS.lock();
    let (mount_id, relative_path) = resolve_path_locked(&vfs, path)?;
    let mount = vfs.mounts.get(mount_id).ok_or(VfsError::NotFound)?;

    let root_inode = mount.fs.root_inode();
    let dir_inode = if relative_path.is_empty() || relative_path == "/" {
        root_inode
    } else {
        walk_path(&*mount.fs, &relative_path)?
    };

    let mut written = 0usize;
    let mut idx = 0;
    while written < buf_len {
        let entry = match mount.fs.readdir(dir_inode, idx)? {
            Some(e) => e,
            None => break,
        };
        idx += 1;
        let entry_bytes = entry.name.as_bytes();
        if written + entry_bytes.len() > buf_len {
            break;
        }
        buf[written..written + entry_bytes.len()].copy_from_slice(entry_bytes);
        written += entry_bytes.len();
        // Add separator (comma)
        if written < buf_len {
            buf[written] = b',';
            written += 1;
        }
    }
    if written > 0 && buf[written - 1] == b',' {
        written -= 1; // Remove trailing comma
    }
    Ok(written)
}

/// Get metadata for a path
pub fn stat(path: &str) -> Result<VfsMetadata, VfsError> {
    let vfs = VFS.lock();
    let (mount_id, relative_path) = resolve_path_locked(&vfs, path)?;
    let mount = vfs.mounts.get(mount_id).ok_or(VfsError::NotFound)?;

    let root_inode = mount.fs.root_inode();
    let inode = if relative_path.is_empty() || relative_path == "/" {
        root_inode
    } else {
        walk_path(&*mount.fs, &relative_path)?
    };
    mount.fs.metadata(inode)
}

/// Remove a file at path
pub fn unlink(path: &str) -> Result<(), VfsError> {
    let mut vfs = VFS.lock();
    let (mount_id, relative_path) = resolve_path_locked(&vfs, path)?;
    if relative_path.is_empty() || relative_path == "/" {
        return Err(VfsError::PermissionDenied);
    }
    let (parent_path, file_name) = split_filename(&relative_path)?;
    let root_inode = {
        let mount = vfs.mounts.get(mount_id).ok_or(VfsError::NotFound)?;
        mount.fs.root_inode()
    };
    let parent_inode = if parent_path.is_empty() || parent_path == "/" {
        root_inode
    } else {
        let mount = vfs.mounts.get(mount_id).ok_or(VfsError::NotFound)?;
        walk_path(&*mount.fs, parent_path)?
    };
    let mount = vfs.mounts.get_mut(mount_id).ok_or(VfsError::NotFound)?;
    mount.fs.unlink(parent_inode, file_name)
}

/// Create a directory at path
pub fn mkdir(path: &str) -> Result<(), VfsError> {
    let mut vfs = VFS.lock();
    let (mount_id, relative_path) = resolve_path_locked(&vfs, path)?;
    if relative_path.is_empty() || relative_path == "/" {
        return Err(VfsError::AlreadyExists);
    }
    let (parent_path, dir_name) = split_filename(&relative_path)?;
    let root_inode = {
        let mount = vfs.mounts.get(mount_id).ok_or(VfsError::NotFound)?;
        mount.fs.root_inode()
    };
    let parent_inode = if parent_path.is_empty() || parent_path == "/" {
        root_inode
    } else {
        let mount = vfs.mounts.get(mount_id).ok_or(VfsError::NotFound)?;
        walk_path(&*mount.fs, parent_path)?
    };
    let mount = vfs.mounts.get_mut(mount_id).ok_or(VfsError::NotFound)?;
    mount.fs.create_dir(parent_inode, dir_name)?;
    Ok(())
}

/// Get a locked reference to the global VFS state
pub fn global_vfs() -> crate::sync::spinlock::SpinLockGuard<'static, VfsState> {
    VFS.lock()
}

// ─── Internal: Path Resolution ────────────────────────────────────────────

/// Internal implementation that doesn't take the lock
fn resolve_path_locked(vfs: &VfsState, path: &str) -> Result<(usize, String), VfsError> {
    if path.is_empty() {
        return Err(VfsError::InvalidParam);
    }

    // Strip leading '/'
    let trimmed = path.strip_prefix('/').unwrap_or(path);

    // Find the longest matching mount prefix
    let mut best_mount: Option<(usize, &str, &str)> = None;
    for (i, entry) in vfs.mounts.iter().enumerate() {
        let prefix_str: &str = entry.prefix.as_str();
        let mount_prefix = prefix_str.strip_prefix('/').unwrap_or(prefix_str);
        if mount_prefix.is_empty() {
            // Root mount — always matches
            if best_mount.is_none() {
                best_mount = Some((i, "", trimmed));
            }
        } else if trimmed.starts_with(mount_prefix) {
            // Check that the match is either exact or followed by '/'
            let remainder = &trimmed[mount_prefix.len()..];
            if remainder.is_empty() || remainder.starts_with('/') {
                let rel = remainder.strip_prefix('/').unwrap_or(remainder);
                // Prefer the longest match
                if best_mount
                    .as_ref()
                    .map_or(true, |(_, prefix, _)| prefix.len() < mount_prefix.len())
                {
                    best_mount = Some((i, mount_prefix, rel));
                }
            }
        }
    }

    match best_mount {
        Some((mount_id, _, relative)) => Ok((mount_id, String::from(relative))),
        None => Err(VfsError::NotFound),
    }
}

// ─── Internal: Path Walking ───────────────────────────────────────────────

/// Walk a path like "dir1/dir2/file" through the filesystem, returning the final inode.
fn walk_path(fs: &dyn FileSystem, path: &str) -> Result<u64, VfsError> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Ok(fs.root_inode());
    }

    let mut inode = fs.root_inode();
    for part in &parts {
        inode = fs.lookup(inode, part)?;
    }
    Ok(inode)
}

// ─── Internal: Utility ────────────────────────────────────────────────────

/// Split "/dir1/dir2/file" into ("/dir1/dir2", "file")
fn split_filename(path: &str) -> Result<(&str, &str), VfsError> {
    if path.is_empty() {
        return Err(VfsError::InvalidParam);
    }
    // Find the last '/'
    if let Some(pos) = path.rfind('/') {
        let parent = &path[..pos];
        let name = &path[pos + 1..];
        if name.is_empty() {
            return Err(VfsError::InvalidParam);
        }
        Ok((parent, name))
    } else {
        Ok(("", path))
    }
}

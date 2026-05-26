// kernel/src/driver/fs.rs — Simple in-memory file system

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::sync::spinlock::SpinLock;

/// A single file with a name and binary data.
#[derive(Debug, Clone)]
pub struct File {
    pub name: String,
    pub data: Vec<u8>,
}

/// A simple in-memory file system.
///
/// Files are stored as a flat list. This is not persistent across reboots,
/// but provides a working file abstraction for the kernel.
pub struct FileSystem {
    files: Vec<File>,
}

/// Global file system instance, protected by a spinlock.
static FS: SpinLock<FileSystem> = SpinLock::new(FileSystem::new_internal());

impl FileSystem {
    /// Internal constructor used for static initialization.
    const fn new_internal() -> Self {
        Self {
            files: Vec::new(),
        }
    }

    /// Create a new empty file system.
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
        }
    }

    /// Create a new empty file with the given name.
    ///
    /// Returns `Ok(())` if the file was created, or `Err(())` if a file
    /// with that name already exists.
    pub fn create(&mut self, name: &str) -> Result<(), ()> {
        if self.files.iter().any(|f| f.name == name) {
            return Err(());
        }
        self.files.push(File {
            name: String::from(name),
            data: Vec::new(),
        });
        Ok(())
    }

    /// Write data to a file. If the file does not exist, it is created.
    pub fn write(&mut self, name: &str, data: &[u8]) -> Result<(), ()> {
        if let Some(file) = self.files.iter_mut().find(|f| f.name == name) {
            file.data.clear();
            file.data.extend_from_slice(data);
            Ok(())
        } else {
            self.files.push(File {
                name: String::from(name),
                data: Vec::from(data),
            });
            Ok(())
        }
    }

    /// Append data to an existing file.
    ///
    /// Returns `Ok(())` if the file exists and data was appended,
    /// or `Err(())` if the file does not exist.
    pub fn append(&mut self, name: &str, data: &[u8]) -> Result<(), ()> {
        if let Some(file) = self.files.iter_mut().find(|f| f.name == name) {
            file.data.extend_from_slice(data);
            Ok(())
        } else {
            Err(())
        }
    }

    /// Read the contents of a file by name.
    pub fn read(&self, name: &str) -> Option<&[u8]> {
        self.files.iter().find(|f| f.name == name).map(|f| f.data.as_slice())
    }

    /// Delete a file by name.
    ///
    /// Returns `Ok(())` if the file was removed, or `Err(())` if not found.
    pub fn delete(&mut self, name: &str) -> Result<(), ()> {
        let len_before = self.files.len();
        self.files.retain(|f| f.name != name);
        if self.files.len() < len_before {
            Ok(())
        } else {
            Err(())
        }
    }

    /// List all files in the file system.
    pub fn list(&self) -> &[File] {
        self.files.as_slice()
    }

    /// Get the number of files in the file system.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

/// Get a locked reference to the global file system.
pub fn global_fs() -> crate::sync::spinlock::SpinLockGuard<'static, FileSystem> {
    FS.lock()
}

/// Initialize the file system.
pub fn init() {
    crate::console_println!("[fs] In-memory file system initialized");
}

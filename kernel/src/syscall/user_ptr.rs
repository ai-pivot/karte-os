//! Type-safe user-space memory access for x86_64.
//!
//! On x86_64, syscalls run under kernel CR3. User-space memory is only
//! accessible under user CR3. These types enforce at **compile time**
//! that every access to user memory goes through the correct CR3 switch.
//!
//! # Design
//!
//! ```text
//! UserPtr<T>   — pointer to a single user-space value
//! UserSliceMut — mutable slice of user memory (for read/write)
//! UserSlice    — immutable slice of user memory (for read only)
//! ```
//!
//! These types are **zero-cost** — they're just `usize` addresses at runtime.
//! All methods handle CR3 switching internally.

use alloc::vec::Vec;

/// A pointer to a single value in user space.
///
/// Cannot be dereferenced directly — must use `.read()` / `.write()`
/// which handle CR3 switching on x86_64.
#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub struct UserPtr<T> {
    addr: usize,
    _marker: core::marker::PhantomData<T>,
}

impl<T: Copy + Default> UserPtr<T> {
    /// Create a UserPtr from a raw address. Returns None if addr is 0.
    #[inline]
    pub fn new(addr: usize) -> Option<Self> {
        if addr == 0 {
            None
        } else {
            Some(Self {
                addr,
                _marker: core::marker::PhantomData,
            })
        }
    }

    /// Create a UserPtr without checking for null.
    #[inline]
    pub fn new_unchecked(addr: usize) -> Self {
        Self {
            addr,
            _marker: core::marker::PhantomData,
        }
    }

    /// Raw address.
    #[inline]
    pub fn addr(&self) -> usize {
        self.addr
    }

    /// Read a value from user space (with CR3 switch on x86_64).
    #[inline]
    pub fn read(&self) -> T {
        let mut val = T::default();
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            val = unsafe { core::ptr::read_volatile(self.addr as *const T) };
        });
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            val = core::ptr::read_volatile(self.addr as *const T);
        }
        val
    }

    /// Write a value to user space (with CR3 switch on x86_64).
    #[inline]
    pub fn write(&self, val: T) {
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            unsafe { core::ptr::write_volatile(self.addr as *mut T, val) };
        });
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            core::ptr::write_volatile(self.addr as *mut T, val)
        }
    }

    /// Offset the pointer by `count` elements.
    #[inline]
    pub fn add(&self, count: usize) -> Self {
        Self {
            addr: self.addr + count * core::mem::size_of::<T>(),
            _marker: core::marker::PhantomData,
        }
    }
}

/// A borrowed slice of user memory (read-only).
#[derive(Debug, Clone, Copy)]
pub struct UserSlice {
    addr: usize,
    len: usize,
}

impl UserSlice {
    /// Create from raw address and length. Returns None if addr is 0 or len is 0.
    #[inline]
    pub fn new(addr: usize, len: usize) -> Option<Self> {
        if addr == 0 || len == 0 || len > 65536 {
            None
        } else {
            Some(Self { addr, len })
        }
    }

    /// Raw address.
    #[inline]
    pub fn addr(&self) -> usize {
        self.addr
    }

    /// Length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Read all bytes from user space into a Vec.
    pub fn read_to_vec(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.len);
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            for i in 0..self.len {
                let byte = unsafe { core::ptr::read_volatile((self.addr + i) as *const u8) };
                buf.push(byte);
            }
        });
        #[cfg(not(target_arch = "x86_64"))]
        for i in 0..self.len {
            let byte = unsafe { core::ptr::read_volatile((self.addr + i) as *const u8) };
            buf.push(byte);
        }
        buf
    }

    /// Read a NUL-terminated string from user space (max 4096 bytes).
    pub fn read_cstring(&self) -> Option<alloc::string::String> {
        let mut buf = Vec::new();
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            for i in 0..self.len.min(4096) {
                let byte = unsafe { core::ptr::read_volatile((self.addr + i) as *const u8) };
                if byte == 0 {
                    return;
                }
                buf.push(byte);
            }
        });
        #[cfg(not(target_arch = "x86_64"))]
        for i in 0..self.len.min(4096) {
            let byte = unsafe { core::ptr::read_volatile((self.addr + i) as *const u8) };
            if byte == 0 {
                break;
            }
            buf.push(byte);
        }
        alloc::string::String::from_utf8(buf).ok()
    }
}

/// A mutable slice of user memory (read-write).
#[derive(Debug, Clone, Copy)]
pub struct UserSliceMut {
    addr: usize,
    len: usize,
}

impl UserSliceMut {
    /// Create from raw address and length. Returns None if addr is 0 or len is 0.
    #[inline]
    pub fn new(addr: usize, len: usize) -> Option<Self> {
        if addr == 0 || len == 0 || len > 65536 {
            None
        } else {
            Some(Self { addr, len })
        }
    }

    /// Raw address.
    #[inline]
    pub fn addr(&self) -> usize {
        self.addr
    }

    /// Length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Read all bytes from user space into a Vec.
    pub fn read_to_vec(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.len);
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            for i in 0..self.len {
                let byte = unsafe { core::ptr::read_volatile((self.addr + i) as *const u8) };
                buf.push(byte);
            }
        });
        #[cfg(not(target_arch = "x86_64"))]
        for i in 0..self.len {
            let byte = unsafe { core::ptr::read_volatile((self.addr + i) as *const u8) };
            buf.push(byte);
        }
        buf
    }

    /// Write bytes to user space from a source slice.
    pub fn copy_from_slice(&self, src: &[u8]) {
        let n = src.len().min(self.len);
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            for i in 0..n {
                unsafe { core::ptr::write_volatile((self.addr + i) as *mut u8, src[i]) };
            }
        });
        #[cfg(not(target_arch = "x86_64"))]
        for i in 0..n {
            unsafe { core::ptr::write_volatile((self.addr + i) as *mut u8, src[i]) };
        }
    }

    /// Write a single byte to user space at offset.
    #[inline]
    pub fn write_byte_at(&self, offset: usize, byte: u8) {
        if offset >= self.len {
            return;
        }
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            unsafe { core::ptr::write_volatile((self.addr + offset) as *mut u8, byte) };
        });
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            core::ptr::write_volatile((self.addr + offset) as *mut u8, byte)
        }
    }
}

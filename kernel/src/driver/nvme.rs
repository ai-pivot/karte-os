//! NVMe (Non-Volatile Memory Express) driver for x86_64.
//!
//! Supports PCIe NVMe SSDs via admin/IO queue pairs.
//! Implements the BlockDevice trait for integration with the filesystem layer.
//!
//! NVMe uses a multi-queue design:
//! - Admin Queue (AQ): controller management (identify, create/delete IO queues)
//! - IO Submission Queue (SQ): host submits read/write commands
//! - IO Completion Queue (CQ): device posts completion status
//!
//! Each queue is a circular buffer in physically contiguous memory.

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::mem::size_of;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::driver::block::{BlockDevice, VfsError};
use crate::sync::spinlock::SpinLock;

// ─── NVMe Register Offsets (from controller BAR0) ─────────────────────────

const NVME_CAP: usize = 0x00; // Controller Capabilities (64-bit)
const NVME_VS: usize = 0x08; // Version (32-bit)
const NVME_INTMS: usize = 0x0C; // Interrupt Mask Set
const NVME_INTMC: usize = 0x10; // Interrupt Mask Clear
const NVME_CC: usize = 0x14; // Controller Configuration
const NVME_CSTS: usize = 0x1C; // Controller Status
const NVME_AQA: usize = 0x24; // Admin Queue Attributes
const NVME_ASQ: usize = 0x28; // Admin Submission Queue Base (64-bit)
const NVME_ACQ: usize = 0x30; // Admin Completion Queue Base (64-bit)

// CC bits
const CC_EN: u32 = 1 << 0;
const CC_CSS_NVM: u32 = 0 << 4; // NVM I/O Command Set

// CSTS bits
const CSTS_RDY: u32 = 1 << 0;
const CSTS_CFS: u32 = 1 << 1;

// CAP bits
fn cap_mqes(cap: u64) -> u16 {
    (cap & 0xFFFF) as u16
}
fn cap_dstrd(cap: u64) -> u8 {
    ((cap >> 32) & 0xF) as u8
}
fn cap_to(cap: u64) -> u8 {
    ((cap >> 24) & 0xFF) as u8
}

// ─── NVMe Command Opcodes ────────────────────────────────────────────────

const OPC_DELETE_IO_SQ: u8 = 0x00;
const OPC_CREATE_IO_SQ: u8 = 0x01;
const OPC_DELETE_IO_CQ: u8 = 0x04;
const OPC_CREATE_IO_CQ: u8 = 0x05;
const OPC_IDENTIFY: u8 = 0x06;
const OPC_SET_FEATURES: u8 = 0x09;
const OPC_READ: u8 = 0x02;
const OPC_WRITE: u8 = 0x01;

// ─── NVMe Command Structures ─────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NvmeCmd {
    cdw0: u32,
    nsid: u32,
    cdw2: u32,
    cdw3: u32,
    mptr: u64,
    dptr1: u64,
    dptr2: u64,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
    cdw14: u32,
    cdw15: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NvmeCompletion {
    cdw0: u32,
    rsvd: u32,
    sq_head: u16, // SQ head pointer at completion
    sq_id: u16,   // SQ ID
    cid: u16,     // Command ID
    status: u16,  // Status field (phase + SC + SCT)
}

// ─── Admin Identify Command Structures ────────────────────────────────────

/// Identify Controller data (CNS=0x01), first 512 bytes
#[repr(C)]
struct IdentifyController {
    vid: u16,   // PCI Vendor ID
    ssvid: u16, // PCI Subsystem Vendor ID
    serial: [u8; 20],
    model: [u8; 40],
    firmware: [u8; 8],
    rab: u8,
    ieee: [u8; 3],
    cmic: u8,
    mdts: u8, // Maximum Data Transfer Size (log2)
    _rsvd: [u8; 128 + 16 + 16 + 8],
    // NN at offset 516 (byte 0x204)
    nn: u32, // Number of Namespaces
             // ... many more fields, we only need nn
}

/// Identify Namespace data (CNS=0x00)
#[repr(C)]
struct IdentifyNamespace {
    nsze: u64, // Namespace Size (in logical blocks)
    ncap: u64, // Namespace Capacity
    nuse: u64, // Namespace Utilization
    nsfeat: u8,
    nlbaf: u8, // Number of LBA Formats - 1
    flbas: u8, // Formatted LBA Size
    // ... rest omitted
    lbaf0: u32, // LBA format 0: bits 23:16 = lbads (logical block size = 2^lbads)
                // ... more fields
}

// ─── Queue Management ────────────────────────────────────────────────────

const QUEUE_DEPTH: usize = 64;

// Safety: Queue memory is exclusively owned by the NVMe controller,
// which is protected by a SpinLock. Raw pointers are only accessed
// while holding the lock.
unsafe impl Send for NvmeQueue {}
unsafe impl Sync for NvmeQueue {}

struct NvmeQueue {
    sq: *mut NvmeCmd,        // Submission queue (physically contiguous)
    cq: *mut NvmeCompletion, // Completion queue
    sq_base_paddr: u64,
    cq_base_paddr: u64,
    sq_tail: u16,
    cq_head: u16,
    cq_phase: u8, // Phase tag (toggles each cycle)
    sq_id: u16,
    cq_id: u16,
    doorbell_sq: *mut u32, // Doorbell register for SQ
    doorbell_cq: *mut u32, // Doorbell register for CQ
}

impl NvmeQueue {
    fn new(sq_id: u16, cq_id: u16) -> Self {
        Self {
            sq: ptr::null_mut(),
            cq: ptr::null_mut(),
            sq_base_paddr: 0,
            cq_base_paddr: 0,
            sq_tail: 0,
            cq_head: 0,
            cq_phase: 1,
            sq_id,
            cq_id,
            doorbell_sq: ptr::null_mut(),
            doorbell_cq: ptr::null_mut(),
        }
    }

    /// Allocate queue memory from PMM and map it.
    /// Returns Ok(()) on success.
    fn alloc_buffers(&mut self) -> Result<(), &'static str> {
        // Allocate SQ: QUEUE_DEPTH entries × sizeof(NvmeCmd)
        let sq_size = QUEUE_DEPTH * size_of::<NvmeCmd>();
        // Allocate CQ: QUEUE_DEPTH entries × sizeof(NvmeCompletion)
        let cq_size = QUEUE_DEPTH * size_of::<NvmeCompletion>();

        // Page-aligned allocation
        let sq_size = QUEUE_DEPTH * size_of::<NvmeCmd>();
        let cq_size = QUEUE_DEPTH * size_of::<NvmeCompletion>();
        let sq_pages = (sq_size + 4095) / 4096;
        let cq_pages = (cq_size + 4095) / 4096;

        let sq_paddr = crate::mm::pmm::alloc_contiguous_frames(sq_pages)
            .ok_or("Failed to allocate SQ memory")?;
        let cq_paddr = crate::mm::pmm::alloc_contiguous_frames(cq_pages)
            .ok_or("Failed to allocate CQ memory")?;

        self.sq_base_paddr = sq_paddr as u64;
        self.cq_base_paddr = cq_paddr as u64;

        // Identity-map the physical addresses to virtual addresses
        // On x86_64, physical memory is identity-mapped in the kernel
        self.sq = sq_paddr as *mut NvmeCmd;
        self.cq = cq_paddr as *mut NvmeCompletion;

        // Zero out queues
        unsafe {
            ptr::write_bytes(self.sq as *mut u8, 0, sq_size);
            ptr::write_bytes(self.cq as *mut u8, 0, cq_size);
        }

        Ok(())
    }

    /// Submit a command to the SQ and ring doorbell.
    unsafe fn submit(&mut self, cmd: NvmeCmd) -> u16 {
        let cid = self.sq_tail;
        (*self.sq.add(self.sq_tail as usize)) = cmd;
        self.sq_tail = (self.sq_tail + 1) % QUEUE_DEPTH as u16;
        // Ring doorbell
        ptr::write_volatile(self.doorbell_sq, self.sq_tail as u32);
        cid
    }

    /// Wait for the next completion on this queue.
    /// Returns the completion entry.
    unsafe fn wait_for_completion(&mut self) -> NvmeCompletion {
        loop {
            let cqe = *self.cq.add(self.cq_head as usize);
            let phase = (cqe.status & 1) as u8;
            if phase == self.cq_phase {
                // Advance CQ head
                self.cq_head = (self.cq_head + 1) % QUEUE_DEPTH as u16;
                if self.cq_head == 0 {
                    self.cq_phase ^= 1; // Toggle phase
                }
                // Ring CQ doorbell
                ptr::write_volatile(self.doorbell_cq, self.cq_head as u32);
                return cqe;
            }
            // Spin-wait with pause
            core::arch::asm!("pause");
        }
    }
}

// ─── NVMe Controller ─────────────────────────────────────────────────────

// Safety: Controller is protected by SpinLock, all pointer access is synchronized.
unsafe impl Send for NvmeController {}
unsafe impl Sync for NvmeController {}

struct NvmeController {
    bar0: usize,          // MMIO base address of controller registers
    doorbell_stride: u32, // Doorbell stride (in dwords)
    ns_count: u32,        // Number of namespaces
    ns_size: u64,         // Namespace size in blocks
    block_size: u32,      // Logical block size (typically 512)
    // Admin queue (queue 0)
    admin_q: NvmeQueue,
    // IO queue (queue 1 SQ, queue 1 CQ)
    io_q: NvmeQueue,
    // PRP (Physical Region Page) buffer for data transfers
    prp_buf_paddr: u64,
    prp_buf: *mut u8,
}

impl NvmeController {
    fn new(bar0: usize) -> Self {
        Self {
            bar0,
            doorbell_stride: 0,
            ns_count: 0,
            ns_size: 0,
            block_size: 512,
            admin_q: NvmeQueue::new(0, 0),
            io_q: NvmeQueue::new(1, 1),
            prp_buf_paddr: 0,
            prp_buf: ptr::null_mut(),
        }
    }

    // ── MMIO helpers ──

    unsafe fn read32(&self, offset: usize) -> u32 {
        ptr::read_volatile((self.bar0 + offset) as *const u32)
    }

    unsafe fn write32(&self, offset: usize, val: u32) {
        ptr::write_volatile((self.bar0 + offset) as *mut u32, val);
    }

    unsafe fn read64(&self, offset: usize) -> u64 {
        ptr::read_volatile((self.bar0 + offset) as *const u64)
    }

    unsafe fn write64(&self, offset: usize, val: u64) {
        ptr::write_volatile((self.bar0 + offset) as *mut u64, val);
    }

    /// Initialize the NVMe controller.
    fn init(&mut self) -> Result<(), &'static str> {
        // Read CAP register
        let cap = unsafe { self.read64(NVME_CAP) };
        let mqes = cap_mqes(cap) as usize;
        let _dstrd = cap_dstrd(cap);
        let _to = cap_to(cap);

        crate::console_println!("[nvme] CAP: mqes={}, dstrd={}, to={}", mqes, _dstrd, _to);

        // Read version
        let vs = unsafe { self.read32(NVME_VS) };
        crate::console_println!(
            "[nvme] Version: {}.{}.{}",
            (vs >> 16) & 0xFF,
            (vs >> 8) & 0xFF,
            vs & 0xFF
        );

        // Doorbell stride: 2^(2 + dstrd) bytes = 4 << dstrd dwords
        self.doorbell_stride = 4 << _dstrd;

        // Check if controller is already enabled
        let csts = unsafe { self.read32(NVME_CSTS) };
        if csts & CSTS_CFS != 0 {
            return Err("Controller fatal error");
        }

        // Disable controller (if enabled)
        let cc = unsafe { self.read32(NVME_CC) };
        if cc & CC_EN != 0 {
            unsafe { self.write32(NVME_CC, cc & !CC_EN) };
            // Wait for RDY to clear (timeout = CAP.TO * 500ms)
            let timeout_iters = (_to as usize + 1) * 50000;
            for _ in 0..timeout_iters {
                let csts = unsafe { self.read32(NVME_CSTS) };
                if csts & CSTS_RDY == 0 {
                    break;
                }
            }
            let csts = unsafe { self.read32(NVME_CSTS) };
            if csts & CSTS_RDY != 0 {
                return Err("Controller failed to disable");
            }
        }

        // Set up Admin Queue
        self.admin_q.alloc_buffers()?;
        // Configure AQA: ASQ size = ACQ size = (queue_depth - 1)
        let aqa = ((QUEUE_DEPTH as u32 - 1) << 16) | (QUEUE_DEPTH as u32 - 1);
        unsafe {
            self.write32(NVME_AQA, aqa);
            self.write64(NVME_ASQ, self.admin_q.sq_base_paddr);
            self.write64(NVME_ACQ, self.admin_q.cq_base_paddr);
        }

        // Admin queue doorbells: offset = 0x1000 for SQ0, 0x1000 + stride for CQ0
        let db_base = self.bar0 + 0x1000;
        self.admin_q.doorbell_sq = db_base as *mut u32;
        self.admin_q.doorbell_cq = (db_base + self.doorbell_stride as usize) as *mut u32;

        // Enable controller
        unsafe {
            self.write32(NVME_CC, CC_EN | CC_CSS_NVM);
        }

        // Wait for RDY (timeout = CAP.TO * 500ms)
        let timeout_iters = (_to as usize + 1) * 50000;
        for _ in 0..timeout_iters {
            let csts = unsafe { self.read32(NVME_CSTS) };
            if csts & CSTS_RDY != 0 {
                break;
            }
        }
        let csts = unsafe { self.read32(NVME_CSTS) };
        if csts & CSTS_RDY == 0 {
            return Err("Controller failed to become ready");
        }
        if csts & CSTS_CFS != 0 {
            return Err("Controller fatal status after enable");
        }

        crate::console_println!("[nvme] Controller enabled and ready");

        // Identify Controller
        let ctrl_data = self.identify_controller()?;
        self.ns_count = ctrl_data.nn;
        let model_str = unsafe { core::str::from_utf8_unchecked(&ctrl_data.model[..40]) };
        crate::console_println!("[nvme] Model: {:?}", model_str.trim());
        crate::console_println!("[nvme] Namespaces: {}", self.ns_count);

        if self.ns_count == 0 {
            return Err("No NVMe namespaces found");
        }

        // Identify Namespace 1
        let ns_data = self.identify_namespace(1)?;
        self.ns_size = ns_data.nsze;
        // Extract LBA data size from FLBAF0 (byte at offset 26 = 0x1A)
        let lbaf0_byte: u8 = unsafe {
            let ptr = &ns_data as *const IdentifyNamespace as *const u8;
            ptr.add(26).read()
        };
        let lbads = lbaf0_byte; // Bits 23:16 of LBA Format 0 descriptor
        self.block_size = if lbads >= 9 && lbads <= 16 {
            1u32 << lbads
        } else {
            512 // Default
        };
        crate::console_println!(
            "[nvme] NS 1: {} blocks × {} bytes = {} MB",
            self.ns_size,
            self.block_size,
            self.ns_size * self.block_size as u64 / (1024 * 1024)
        );

        // Create IO Completion Queue (CQ 1)
        self.io_q.alloc_buffers()?;
        self.admin_create_io_cq()?;

        // Create IO Submission Queue (SQ 1 → CQ 1)
        self.admin_create_io_sq()?;

        // Set IO queue doorbells
        // SQ1 doorbell = db_base + 2 * stride (SQ0=0, CQ0=1, SQ1=2, CQ1=3)
        let sq1_db = db_base + 2 * self.doorbell_stride as usize;
        let cq1_db = db_base + 3 * self.doorbell_stride as usize;
        self.io_q.doorbell_sq = sq1_db as *mut u32;
        self.io_q.doorbell_cq = cq1_db as *mut u32;

        // Allocate PRP buffer for data transfers (at least one page)
        let prp_paddr = crate::mm::pmm::alloc_contiguous_frames(2)
            .ok_or("Failed to allocate NVMe PRP buffer")?;
        self.prp_buf_paddr = prp_paddr as u64;
        self.prp_buf = prp_paddr as *mut u8;

        crate::console_println!("[nvme] Initialization complete");

        Ok(())
    }

    // ── Admin Commands ──

    fn admin_create_io_cq(&mut self) -> Result<(), &'static str> {
        let cmd = NvmeCmd {
            cdw0: (OPC_CREATE_IO_CQ as u32),
            nsid: 0,
            cdw10: ((QUEUE_DEPTH as u32 - 1) << 16) | 1, // Q size & Q ID=1
            cdw11: (1 << 0) | (1 << 1), // Physically Contiguous + Interrupt Enabled
            dptr1: self.io_q.cq_base_paddr,
            ..Default::default()
        };
        let cqe = unsafe {
            self.admin_q.submit(cmd);
            self.admin_q.wait_for_completion()
        };
        let sc = (cqe.status >> 1) & 0xFF;
        if sc != 0 {
            return Err("Failed to create IO CQ");
        }
        crate::console_println!("[nvme] IO CQ 1 created");
        Ok(())
    }

    fn admin_create_io_sq(&mut self) -> Result<(), &'static str> {
        let cmd = NvmeCmd {
            cdw0: (OPC_CREATE_IO_SQ as u32),
            nsid: 0,
            cdw10: ((QUEUE_DEPTH as u32 - 1) << 16) | 1, // Q size & Q ID=1
            cdw11: (1 << 0) | (1 << 16),                 // Physically Contiguous + CQ ID=1
            dptr1: self.io_q.sq_base_paddr,
            ..Default::default()
        };
        let cqe = unsafe {
            self.admin_q.submit(cmd);
            self.admin_q.wait_for_completion()
        };
        let sc = (cqe.status >> 1) & 0xFF;
        if sc != 0 {
            return Err("Failed to create IO SQ");
        }
        crate::console_println!("[nvme] IO SQ 1 created");
        Ok(())
    }

    fn identify_controller(&mut self) -> Result<IdentifyController, &'static str> {
        let buf_paddr = crate::mm::pmm::alloc_contiguous_frames(1)
            .ok_or("Failed to allocate identify buffer")?;

        let cmd = NvmeCmd {
            cdw0: OPC_IDENTIFY as u32,
            cdw10: 0x01, // CNS = 1 (Identify Controller)
            dptr1: buf_paddr as u64,
            ..Default::default()
        };

        unsafe {
            self.admin_q.submit(cmd);
            self.admin_q.wait_for_completion();
        }

        let result = unsafe { ptr::read(buf_paddr as *const IdentifyController) };
        crate::mm::pmm::dealloc_frame(buf_paddr);
        Ok(result)
    }

    fn identify_namespace(&mut self, nsid: u32) -> Result<IdentifyNamespace, &'static str> {
        let buf_paddr = crate::mm::pmm::alloc_contiguous_frames(1)
            .ok_or("Failed to allocate identify buffer")?;

        let cmd = NvmeCmd {
            cdw0: OPC_IDENTIFY as u32,
            nsid,
            cdw10: 0x00, // CNS = 0 (Identify Namespace)
            dptr1: buf_paddr as u64,
            ..Default::default()
        };

        unsafe {
            self.admin_q.submit(cmd);
            self.admin_q.wait_for_completion();
        }

        let result = unsafe { ptr::read(buf_paddr as *const IdentifyNamespace) };
        crate::mm::pmm::dealloc_frame(buf_paddr);
        Ok(result)
    }

    // ── I/O Commands ──

    fn do_read(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), VfsError> {
        let buf_paddr = self.prp_buf_paddr;

        let cmd = NvmeCmd {
            cdw0: OPC_READ as u32,
            nsid: 1,
            dptr1: buf_paddr,
            cdw10: lba as u32,         // Starting LBA (low 32 bits)
            cdw11: (lba >> 32) as u32, // Starting LBA (high 32 bits)
            cdw12: count,              // Number of logical blocks
            ..Default::default()
        };

        unsafe {
            self.io_q.submit(cmd);
            let cqe = self.io_q.wait_for_completion();
            let sc = (cqe.status >> 1) & 0xFF;
            if sc != 0 {
                return Err(VfsError::IoError);
            }
            // Copy from PRP buffer to user buffer
            let byte_count = count as usize * self.block_size as usize;
            ptr::copy_nonoverlapping(self.prp_buf, buf.as_mut_ptr(), byte_count.min(buf.len()));
        }
        Ok(())
    }

    fn do_write(&mut self, lba: u64, count: u32, data: &[u8]) -> Result<(), VfsError> {
        let buf_paddr = self.prp_buf_paddr;

        // Copy data to PRP buffer first
        unsafe {
            let byte_count = count as usize * self.block_size as usize;
            ptr::copy_nonoverlapping(data.as_ptr(), self.prp_buf, byte_count.min(data.len()));
            // Zero-pad if needed
            if data.len() < byte_count {
                ptr::write_bytes(self.prp_buf.add(data.len()), 0, byte_count - data.len());
            }
        }

        let cmd = NvmeCmd {
            cdw0: OPC_WRITE as u32,
            nsid: 1,
            dptr1: buf_paddr,
            cdw10: lba as u32,
            cdw11: (lba >> 32) as u32,
            cdw12: count,
            ..Default::default()
        };

        unsafe {
            self.io_q.submit(cmd);
            let cqe = self.io_q.wait_for_completion();
            let sc = (cqe.status >> 1) & 0xFF;
            if sc != 0 {
                return Err(VfsError::IoError);
            }
        }
        Ok(())
    }
}

// ─── Global NVMe State ───────────────────────────────────────────────────

static NVME_AVAILABLE: AtomicBool = AtomicBool::new(false);

struct NvmeState {
    controller: Option<NvmeController>,
}

static NVME: SpinLock<NvmeState> = SpinLock::new(NvmeState { controller: None });

// ─── BlockDevice Implementation ──────────────────────────────────────────

/// Global NVMe block device wrapper.
pub struct NvmeBlockDevice;

impl BlockDevice for NvmeBlockDevice {
    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> Result<(), VfsError> {
        let mut state = NVME.lock();
        let ctrl = state.controller.as_mut().ok_or(VfsError::NoDevice)?;
        // Convert from 512-byte sectors to NVMe logical blocks
        let lba = if ctrl.block_size == 512 {
            block_id as u64
        } else {
            (block_id as u64 * 512) / ctrl.block_size as u64
        };
        ctrl.do_read(lba, 1, buf)
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) -> Result<(), VfsError> {
        let mut state = NVME.lock();
        let ctrl = state.controller.as_mut().ok_or(VfsError::NoDevice)?;
        let lba = if ctrl.block_size == 512 {
            block_id as u64
        } else {
            (block_id as u64 * 512) / ctrl.block_size as u64
        };
        ctrl.do_write(lba, 1, buf)
    }

    fn block_size(&self) -> usize {
        512 // Always report 512 to the VFS layer
    }

    fn capacity_blocks(&self) -> usize {
        let state = NVME.lock();
        if let Some(ref ctrl) = state.controller {
            let total_bytes = ctrl.ns_size * ctrl.block_size as u64;
            (total_bytes / 512) as usize
        } else {
            0
        }
    }
}

static NVME_BLK_DEV: NvmeBlockDevice = NvmeBlockDevice;

/// Initialize NVMe controller from PCI device BAR0.
/// Called from PCI enumeration if an NVMe device is found.
pub fn init(bar0: usize, bar_size: usize) -> Result<(), &'static str> {
    crate::console_println!(
        "[nvme] Initializing controller at BAR0={:#x}, size={:#x}",
        bar0,
        bar_size
    );

    // Map BAR0 into virtual address space (identity-mapped on x86_64)
    let mut ctrl = NvmeController::new(bar0);

    // Verify we can access the registers
    let cap = unsafe { ctrl.read64(NVME_CAP) };
    if cap == 0 || cap == 0xFFFFFFFFFFFFFFFF {
        return Err("No NVMe controller at BAR0");
    }

    ctrl.init()?;

    // Register as block device
    let mut state = NVME.lock();
    state.controller = Some(ctrl);
    drop(state);

    crate::driver::block::set_block_device(&NVME_BLK_DEV);
    NVME_AVAILABLE.store(true, Ordering::Relaxed);

    Ok(())
}

/// Check if NVMe is available
pub fn is_available() -> bool {
    NVME_AVAILABLE.load(Ordering::Relaxed)
}

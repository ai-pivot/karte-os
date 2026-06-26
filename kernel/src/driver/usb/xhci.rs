//! xHCI host controller driver — full implementation.
//!
//! This replaces the old `driver/xhci.rs` prototype. It implements a proper
//! command/event/transfer ring state machine, Input/Output Context management,
//! and device enumeration via Enable Slot / Address Device / Configure Endpoint.
//!
//! Public API used by the rest of the kernel:
//!   - `init()` — probe and bring up the controller (safe, no enumeration)
//!   - `enumerate_devices()` — enumerate root-hub ports and HID keyboards
//!   - `poll_keyboard()` — polling fallback for interrupt endpoint
//!   - `handle_irq()` — interrupt entry point (advances event ring only)
//!
//! Safety: all ring/context memory is allocated from a typed DMA arena with
//! explicit offsets and bounds checks. The controller is never asked to DMA
//! into or from arbitrary kernel memory.

#![allow(dead_code)]
// Rust 2024 requires explicit `unsafe {}` blocks inside `unsafe fn` bodies
// (RFC 2585, lint `unsafe_op_in_unsafe_fn`). Our MMIO helpers are all
// `unsafe fn` that call other `unsafe fn`; the trust boundary is the
// function signature, so we silence the inner-block requirement here.
#![allow(unsafe_op_in_unsafe_fn)]

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU64, Ordering};

use super::*;

// ─── Capability registers (read via MMIO base) ────────────────────────

const CAP_LENGTH: usize = 0x00; // u8, but we read u32 for alignment
const CAP_HCSPARAMS1: usize = 0x04;
const CAP_HCSPARAMS2: usize = 0x08;
const CAP_HCCPARAMS1: usize = 0x10;
const CAP_DBOFF: usize = 0x14;
const CAP_RTSOFF: usize = 0x18;

// Extended Capability IDs (xHCI spec 7.5, xhci-ext-caps.h).
const XEC_ID_LEGACY: u32 = 1; // USB Legacy Support
const XEC_ID_PROTOCOL: u32 = 2; // Supported Protocol

const XHCI_LEGACY_BIOS_OWNED: u32 = 1 << 16;
const XHCI_LEGACY_OS_OWNED: u32 = 1 << 24;

// ─── Operational registers (offset = caplength) ───────────────────────

const OP_USBCMD: usize = 0x00;
const OP_USBSTS: usize = 0x04;
const OP_PAGESIZE: usize = 0x08;
const OP_CRCR_LO: usize = 0x18;
const OP_CRCR_HI: usize = 0x1C;
const OP_MFINDEX: usize = 0x24;
const OP_DCBAAP_LO: usize = 0x30;
const OP_DCBAAP_HI: usize = 0x34;
const OP_CONFIG: usize = 0x38;

// ─── Port registers (offset = caplength + 0x400 + port*16) ────────────

const PORT_SC_OFFSET: usize = 0x400;

// ─── Runtime registers (offset = RTSOFF) ──────────────────────────────

const RT_IR0: usize = 0x20; // Interrupter 0 base
const RT_IR0_IMAN: usize = RT_IR0 + 0x00;
const RT_IR0_IMOD: usize = RT_IR0 + 0x04;
const RT_IR0_ERSTSZ: usize = RT_IR0 + 0x08;
const RT_IR0_ERSTBA_LO: usize = RT_IR0 + 0x10;
const RT_IR0_ERSTBA_HI: usize = RT_IR0 + 0x14;
const RT_IR0_ERDP_LO: usize = RT_IR0 + 0x18;
const RT_IR0_ERDP_HI: usize = RT_IR0 + 0x1C;

// ─── USBCMD bits ───────────────────────────────────────────────────────

const CMD_RUN: u32 = 1 << 0;
const CMD_HCRST: u32 = 1 << 1;
const CMD_INTE: u32 = 1 << 2;
const CMD_HSEE: u32 = 1 << 3;

// ─── USBSTS bits ───────────────────────────────────────────────────────

const STS_HCH: u32 = 1 << 0;
const STS_CNR: u32 = 1 << 11;

// ─── Port Status & Control bits ────────────────────────────────────────

const PORT_CCS: u32 = 1 << 0; // Current Connect Status
const PORT_PED: u32 = 1 << 1; // Port Enabled/Disabled
const PORT_PR: u32 = 1 << 4; // Port Reset
const PORT_PP: u32 = 1 << 9; // Port Power
const PORT_CSC: u32 = 1 << 17; // Connect Status Change
const PORT_PEC: u32 = 1 << 18; // Port Enabled/Disabled Change
const PORT_WRC: u32 = 1 << 19; // Warm Reset Change
const PORT_OCC: u32 = 1 << 20; // Over-current Change
const PORT_PRC: u32 = 1 << 21; // Port Reset Change
const PORT_PLC: u32 = 1 << 22; // Port Link State Change
const PORT_CEC: u32 = 1 << 23; // Port Config Error Change
const PORT_WPR: u32 = 1 << 31; // Warm Port Reset (USB3 only)
const PORT_PLS_MASK: u32 = 0xF << 5;
const PORT_PLS_SHIFT: usize = 5;
const PORT_SPEED_SHIFT: usize = 10;
const PORT_SPEED_MASK: u32 = 0xF << PORT_SPEED_SHIFT;
const PORT_CHANGE_MASK: u32 =
    PORT_CSC | PORT_PEC | PORT_WRC | PORT_OCC | PORT_PRC | PORT_PLC | PORT_CEC;
// Writable control bits we own. USB2 reset uses PORT_PR; USB3 warm reset uses
// PORT_WPR. Both share PORT_PP. The exact reset bit is selected per-protocol
// in `port_write_preserve`.
const PORT_WRITE_MASK_USB2: u32 = PORT_PR | PORT_PP;
const PORT_WRITE_MASK_USB3: u32 = PORT_WPR | PORT_PP;
const PORT_WRITE_MASK_DEFAULT: u32 = PORT_PR | PORT_PP;

// Port Link State values (xHCI spec 5.4.8, Table 147).
const PLS_U0: u32 = 0;

// ─── Interrupter Management (IMAN) bits ────────────────────────────────

const IMAN_IP: u32 = 1 << 0; // Interrupt Pending (RW1C)
const IMAN_IE: u32 = 1 << 1; // Interrupt Enable

// ─── TRB types ─────────────────────────────────────────────────────────

const TRB_NORMAL: u32 = 1;
const TRB_SETUP_STAGE: u32 = 2;
const TRB_DATA_STAGE: u32 = 3;
const TRB_STATUS_STAGE: u32 = 4;
const TRB_LINK: u32 = 6;
const TRB_ENABLE_SLOT: u32 = 9;
const TRB_DISABLE_SLOT: u32 = 10;
const TRB_ADDRESS_DEV: u32 = 11;
const TRB_CONFIGURE_EP: u32 = 12;
const TRB_EVALUATE_CTX: u32 = 13;
const TRB_NOOP: u32 = 23;

// Event TRB types
const TRB_TRANSFER_EVENT: u32 = 32;
const TRB_CMD_COMPLETE: u32 = 33;
const TRB_PORT_STATUS_CHANGE: u32 = 34;

const TRB_CYCLE_BIT: u32 = 1;
const TRB_IOC: u32 = 1 << 5; // Interrupt-On-Completion
const TRB_CHAIN: u32 = 1 << 4; // Chain bit
const TRB_ENT: u32 = 1 << 1; // Event TRB Data (Interrupt-on-Completion)
const TRB_ISP: u32 = 1 << 2; // Interrupt on Short Packet
const TRB_IDT: u32 = 1 << 6; // Immediate Data (Setup Stage carries 8-byte setup in parameter)

// Setup Stage TRB Transfer Type (TRT) field, bits 16:17.
const TRB_TRT_NONE: u32 = 0 << 16;
const TRB_TRT_IN: u32 = 2 << 16;
const TRB_TRT_OUT: u32 = 3 << 16;
// Data/Status Stage TRB Direction bit, bit 16.
const TRB_DIR_IN: u32 = 1 << 16;
const TRB_DIR_OUT: u32 = 0;

// TRB completion code field (bits 31:24 of status)
const TRB_CC_SUCCESS: u32 = 1;
const TRB_CC_SHORT_PACKET: u32 = 13;

const TSC_CYCLES_PER_MS: u64 = 1_000_000; // conservative early-boot estimate
const CMD_TIMEOUT_MS: u64 = 1000;
const CONTROL_TRANSFER_TIMEOUT_MS: u64 = 5000;

// ─── TRB structure (16 bytes) ──────────────────────────────────────────

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct Trb {
    pub parameter: u64,
    pub status: u32,
    pub control: u32,
}

impl Trb {
    pub const fn zero() -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: 0,
        }
    }

    /// Build a TRB control word from its type (10:15), cycle bit (0), and any
    /// additional flag bits. Centralizes the `(type << 10) | flags` encoding so
    /// every call site is consistent and the type field is never accidentally
    /// OR'd in as a low-order constant.
    pub const fn control(trb_type: u32, flags: u32) -> u32 {
        (trb_type << 10) | flags
    }

    pub fn trb_type(&self) -> u32 {
        (self.control >> 10) & 0x3F
    }

    pub fn cycle(&self) -> bool {
        self.control & TRB_CYCLE_BIT != 0
    }

    pub fn completion_code(&self) -> u32 {
        (self.status >> 24) & 0xFF
    }

    pub fn slot_id(&self) -> u8 {
        ((self.control >> 24) & 0xFF) as u8
    }

    pub fn endpoint_id(&self) -> u8 {
        ((self.control >> 16) & 0x1F) as u8
    }
}

// ─── ERST entry (16 bytes) ─────────────────────────────────────────────

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ErstEntry {
    pub ring_seg_addr_lo: u32,
    pub ring_seg_addr_hi: u32,
    pub ring_seg_size: u32,
    pub reserved: u32,
}

// ─── Input/Output Context layout (xHCI §6.2) ──────────────────────────
//
// Input Context = Input Control Context (A0/A1 flags) + Slot Context
// + up to 31 Endpoint Contexts.
// Output Context = Slot Context + Endpoint Contexts.
//
// Context size is controller-dependent: HCCPARAMS1.CSZ=0 means 32 bytes,
// CSZ=1 means 64 bytes. QEMU commonly uses 32; many real controllers use 64.
// Use max-sized DMA regions and compute offsets at runtime.

const MAX_CONTEXT_SIZE: usize = 64;
const MAX_ENDPOINTS: usize = 31; // DCI 1..31
const INPUT_CONTEXT_SIZE: usize = MAX_CONTEXT_SIZE * (1 + 1 + MAX_ENDPOINTS);
const OUTPUT_CONTEXT_SIZE: usize = MAX_CONTEXT_SIZE * (1 + MAX_ENDPOINTS);

// Dword indices within the Input Context (each dword = 4 bytes).
// Input Control Context dwords (xHCI spec 6.2.5.1).
// dword 0 = Drop Context flags (which contexts to disable).
// dword 1 = Add Context flags  (which contexts to enable).
const ICTX_A0: usize = 0; // drop flags
const ICTX_A1: usize = 1; // add flags
// Slot Context starts after the Input Control Context.
fn slot_ctx_dword() -> usize {
    context_dwords()
}

// Endpoint Context for DCI n starts after Slot Context plus n endpoint contexts.
fn ep_ctx_dword(dci: u8) -> usize {
    slot_ctx_dword() + context_dwords() * dci as usize
}

fn output_ep_ctx_dword(dci: u8) -> usize {
    context_dwords() * dci as usize
}

// Slot Context field offsets (dword index within the 32-byte context).
// Layout per xHCI spec table 6-4..6-7:
//   dword 0 (dev_info):  route string (0-19), speed (20-23), MTT (25),
//                        hub (26), context entries (27-31).
//   dword 1 (dev_info2): max exit latency (0-15), root hub port (16-23),
//                        num ports (24-31, hub only).
//   dword 2 (tt_info):   interrupter target (0-15) for non-hub devices.
//   dword 3 (dev_state): slot state (0-27), device address (28-31).
const SLOT_DEV_INFO: usize = 0;
const SLOT_DEV_INFO2: usize = 1;
const SLOT_TT_INFO: usize = 2;
const SLOT_DEV_STATE: usize = 3;

// Endpoint Context field offsets (dword index within the 32-byte context).
// Layout per xHCI spec table 6-8..6-11 (matches Linux xhci_ep_ctx):
//   dword 0 (ep_info):  EP state (0-2), mult (8-9), interval (16-23),
//                        max ESIT payload hi (24-31).
//   dword 1 (ep_info2): CErr (1-2), EP type (3-5), max burst (8-15),
//                        max packet size (16-31).
//   dword 2..3 (deq):   64-bit TR dequeue pointer, DCS=bit0 of dword 2.
//   dword 4 (tx_info):  avg TRB length (0-15), max ESIT payload (16-31).
const EP_INFO: usize = 0; // interval in bits 16-23
const EP_INFO2: usize = 1; // cerr/ep_type/max_burst/max_packet
const EP_DEQ_LO: usize = 2;
const EP_DEQ_HI: usize = 3;
const EP_TX_INFO: usize = 4; // avg TRB length in bits 0-15
const EP_STATE_RUNNING: u32 = 1;

// Endpoint types (EP_TYPE field)
const EP_TYPE_CONTROL_OUT: u32 = 4;
const EP_TYPE_BULK_OUT: u32 = 2;
const EP_TYPE_BULK_IN: u32 = 6;
const EP_TYPE_INT_OUT: u32 = 3;
const EP_TYPE_INT_IN: u32 = 7;
const EP_TYPE_ISOC_OUT: u32 = 1;
const EP_TYPE_ISOC_IN: u32 = 5;

// ─── DMA arena layout ──────────────────────────────────────────────────
//
// One contiguous physical arena, partitioned into fixed regions. All offsets
// are page multiples so each ring/context starts page-aligned.
//
//   page 0        DCBAA (MaxSlots+1 pointers)
//   page 1        ERST (1 entry, 16 bytes)
//   page 2        Event Ring segment (256 TRBs)
//   page 3        Command Ring (256 TRBs + link)
//   page 4        Input Context (input control + slot + 31 EPs)
//   page 5..10    reserved/fixed transfer rings and data buffers
//   page 11..42   Per-slot Output Contexts (64 slots × 2KB max context)
//   page 7        Control Transfer Ring (per-device, 256 TRBs + link)
//   page 8        Control Data Buffer (4096 bytes)
//   page 9        Keyboard Interrupt Transfer Ring (256 TRBs + link)
//   page 10       HID report buffer (4096 bytes)
//   page 43..74   Per-DCI interrupt rings for non-keyboard endpoints
//   page 75..79   reserved for additional devices/hub
//
// This layout keeps every ring/context within the arena; no out-of-bounds
// DMA can touch kernel memory.

const DCBAA_OFF: usize = 0;
const ERST_OFF: usize = 4096;
const EVENT_RING_OFF: usize = 4096 * 2;
const CMD_RING_OFF: usize = 4096 * 3;
const INPUT_CTX_OFF: usize = 4096 * 4;
const CTRL_RING_OFF: usize = 4096 * 7;
const CTRL_DATA_OFF: usize = 4096 * 8;
const INT_RING_OFF: usize = 4096 * 9;
const INT_DATA_OFF: usize = 4096 * 10;
const OUTPUT_CTX_OFF: usize = 4096 * 11;
const EP_RING_BASE_OFF: usize = 4096 * 43;
const MAX_SUPPORTED_SLOTS: usize = 64;
const DMA_PAGES: usize = 80;

const RING_SIZE: usize = 256; // TRBs per ring segment
const RING_BYTES: usize = RING_SIZE * core::mem::size_of::<Trb>();

// ─── Global controller state ───────────────────────────────────────────

static XHCI_MMIO: AtomicU64 = AtomicU64::new(0);
static XHCI_OPBASE: AtomicU64 = AtomicU64::new(0);
static XHCI_DBBASE: AtomicU64 = AtomicU64::new(0);
static XHCI_RTBASE: AtomicU64 = AtomicU64::new(0);
static XHCI_MAX_PORTS: AtomicU64 = AtomicU64::new(0);
static XHCI_MAX_SLOTS: AtomicU64 = AtomicU64::new(0);
static XHCI_CONTEXT_SIZE: AtomicU64 = AtomicU64::new(32);
static XHCI_READY: AtomicBool = AtomicBool::new(false);

// Per-port protocol info, indexed by 1-based logical port number (1..=MaxPorts).
// PORT_MAJOR_REV: 0 = unknown/not declared, 2 = USB2, 3 = USB3.
// PORT_MMIO_IDX: 0-based index into the contiguous port register set
//   (PORTSC at opbase + 0x400 + idx*0x10). For controllers without a
//   Supported Protocol extended capability we fall back to idx = port-1.
// On real hardware USB2 and USB3 root ports are interleaved in arbitrary
// order in the port register set; the Supported Protocol capability is the
// only authoritative source of which logical port speaks which protocol and
// where its PORTSC lives. Scanning 1..=MaxPorts with offset (port-1)*0x10 and
// treating every port as USB2 (PR-bit reset) misses any USB3 keyboard attached
// to a rear port whose logical number maps to a USB3 root port.
const PORT_CAPS_LEN: usize = 256;
static PORT_MAJOR_REV: [AtomicU8; PORT_CAPS_LEN] = [const { AtomicU8::new(0) }; PORT_CAPS_LEN];
static PORT_MMIO_IDX: [AtomicU8; PORT_CAPS_LEN] = [const { AtomicU8::new(0) }; PORT_CAPS_LEN];

static DMA_PHYS: AtomicU64 = AtomicU64::new(0);

// Ring cycle state (producer side for command/control rings; consumer side
// for the event ring). All protected by RING_LOCK during command submission.
static RING_LOCK: crate::sync::spinlock::SpinLock<RingState> =
    crate::sync::spinlock::SpinLock::new(RingState {
        cmd_cycle: true,
        cmd_index: 0,
        evt_cycle: true,
        evt_index: 0,
        ctrl_cycle: true,
        ctrl_index: 0,
        int_cycle: true,
        int_index: 0,
    });

#[derive(Clone, Copy)]
struct RingState {
    cmd_cycle: bool,
    cmd_index: usize,
    evt_cycle: bool,
    evt_index: usize,
    ctrl_cycle: bool,
    ctrl_index: usize,
    int_cycle: bool,
    int_index: usize,
}

// Keyboard state
static KEYBOARD_SLOT: AtomicU8 = AtomicU8::new(0);
static KEYBOARD_EP_DCI: AtomicU8 = AtomicU8::new(0);
static KEYBOARD_IFACE: AtomicU8 = AtomicU8::new(0);
static KEYBOARD_PROTO: AtomicU8 = AtomicU8::new(0);
static KEYBOARD_REPORT_LEN: AtomicU16 = AtomicU16::new(8);
static HAS_KEYBOARD: AtomicBool = AtomicBool::new(false);
static SYNC_ENUM_ACTIVE: AtomicBool = AtomicBool::new(false);
static KBD_LAST_NUDGE_TSC: AtomicU64 = AtomicU64::new(0);
static KBD_LAST_GET_REPORT_TSC: AtomicU64 = AtomicU64::new(0);
static KBD_GET_REPORT_DISABLED: AtomicBool = AtomicBool::new(false);
// True while an interrupt-IN transfer for the HID keyboard is in flight on
// the controller. Set when we enqueue a Normal TRB and ring the doorbell,
// cleared in `handle_irq` when the matching Transfer Event arrives. This
// prevents queueing multiple outstanding transfers and lets the keyboard be
// driven entirely by the XHCI interrupt instead of timer-ISR polling.
static KBD_XFER_PENDING: AtomicBool = AtomicBool::new(false);
// Bit N means root-hub port (N+1) has had at least one status change since
// the last consumer snapshot. ISR updates this without doing enumeration.
static ROOT_PORT_CHANGE_BITS: AtomicU64 = AtomicU64::new(0);

// ─── MMIO helpers ──────────────────────────────────────────────────────

#[inline]
unsafe fn reg_read(offset: usize) -> u32 {
    let addr = (XHCI_MMIO.load(Ordering::Relaxed) as usize) + offset;
    core::ptr::read_volatile(addr as *const u32)
}

#[inline]
unsafe fn reg_write(offset: usize, val: u32) {
    let addr = (XHCI_MMIO.load(Ordering::Relaxed) as usize) + offset;
    core::ptr::write_volatile(addr as *mut u32, val);
}

#[inline]
unsafe fn op_read(offset: usize) -> u32 {
    let addr = (XHCI_OPBASE.load(Ordering::Relaxed) as usize) + offset;
    core::ptr::read_volatile(addr as *const u32)
}

#[inline]
unsafe fn op_write(offset: usize, val: u32) {
    let addr = (XHCI_OPBASE.load(Ordering::Relaxed) as usize) + offset;
    core::ptr::write_volatile(addr as *mut u32, val);
}

#[inline]
unsafe fn rt_read(offset: usize) -> u32 {
    let addr = (XHCI_RTBASE.load(Ordering::Relaxed) as usize) + offset;
    core::ptr::read_volatile(addr as *const u32)
}

#[inline]
unsafe fn rt_write(offset: usize, val: u32) {
    let addr = (XHCI_RTBASE.load(Ordering::Relaxed) as usize) + offset;
    core::ptr::write_volatile(addr as *mut u32, val);
}

/// Return the 0-based MMIO index of a logical port (1-based) into the
/// contiguous port register set. Falls back to `port-1` when the Supported
/// Protocol extended capability has not declared a mapping for this port.
#[inline]
fn port_mmio_index(port: usize) -> usize {
    if port == 0 || port >= PORT_CAPS_LEN {
        return port.saturating_sub(1);
    }
    let idx = PORT_MMIO_IDX[port].load(Ordering::Relaxed);
    if idx == 0 {
        // Uninitialized: fall back to the historical (port-1) layout so QEMU
        // (which has no interleaving) and any controller without ext caps keep
        // working. A real mapping sets explicit indices starting at 0, so the
        // 0 sentinel is unambiguous.
        port - 1
    } else {
        idx as usize
    }
}

/// Return the USB major revision (2 or 3) declared for a logical port, or 0
/// if the Supported Protocol capability did not cover this port. A 0 result
/// is treated as USB2 by callers for backward compatibility with QEMU.
#[inline]
fn port_major_rev(port: usize) -> u8 {
    if port == 0 || port >= PORT_CAPS_LEN {
        return 0;
    }
    PORT_MAJOR_REV[port].load(Ordering::Relaxed)
}

/// Read a port status/control register. `port` is the 1-based logical port
/// number; its MMIO index is resolved through the Supported Protocol map.
#[inline]
unsafe fn port_read(port: usize) -> u32 {
    op_read(PORT_SC_OFFSET + port_mmio_index(port) * 16)
}

/// Write a port status/control register. `port` is the 1-based logical port
/// number; its MMIO index is resolved through the Supported Protocol map.
#[inline]
unsafe fn port_write(port: usize, val: u32) {
    op_write(PORT_SC_OFFSET + port_mmio_index(port) * 16, val)
}

/// Write PORTSC without echoing read-only or write-1-to-clear status bits.
///
/// PORTSC contains mixed semantics: `PED` is a status bit that real xHCI
/// controllers may treat as write-1-to-disable, while change bits are W1C.
/// Only write active controls we own (PP plus the protocol-appropriate reset
/// bit) plus explicit W1C change bits.
#[inline]
unsafe fn port_write_preserve(port: usize, sc: u32, clear_changes: u32) {
    let mask = match port_major_rev(port) {
        3 => PORT_WRITE_MASK_USB3,
        2 => PORT_WRITE_MASK_USB2,
        _ => PORT_WRITE_MASK_DEFAULT,
    };
    port_write(port, (sc & mask) | (clear_changes & PORT_CHANGE_MASK))
}

#[inline]
unsafe fn doorbell_ring(slot: u32, target: u32) {
    let db_addr = (XHCI_DBBASE.load(Ordering::Relaxed) as usize) + (slot as usize) * 4;
    core::ptr::write_volatile(db_addr as *mut u32, target);
    // Flush PCI posted MMIO writes. Linux does the same after ringing xHCI
    // doorbells; without this a doorbell may sit in a posted write buffer while
    // the CPU continues polling memory state that the controller never saw.
    let _ = core::ptr::read_volatile(db_addr as *const u32);
}

#[inline]
fn dma_phys() -> usize {
    DMA_PHYS.load(Ordering::Relaxed) as usize
}

#[inline]
fn context_size() -> usize {
    XHCI_CONTEXT_SIZE.load(Ordering::Relaxed) as usize
}

#[inline]
fn context_dwords() -> usize {
    context_size() / 4
}

#[inline]
fn phys_to_virt(paddr: usize) -> usize {
    crate::mm::vmm::phys_to_virt(paddr)
}

// ─── Ring memory access ────────────────────────────────────────────────

#[inline]
fn cmd_ring_virt() -> *mut Trb {
    phys_to_virt(dma_phys() + CMD_RING_OFF) as *mut Trb
}

#[inline]
fn event_ring_virt() -> *mut Trb {
    phys_to_virt(dma_phys() + EVENT_RING_OFF) as *mut Trb
}

#[inline]
fn ctrl_ring_virt() -> *mut Trb {
    phys_to_virt(dma_phys() + CTRL_RING_OFF) as *mut Trb
}

#[inline]
fn int_ring_virt() -> *mut Trb {
    phys_to_virt(dma_phys() + INT_RING_OFF) as *mut Trb
}

#[inline]
fn endpoint_ring_off(dci: u8) -> usize {
    EP_RING_BASE_OFF + dci as usize * 4096
}

#[inline]
fn endpoint_ring_virt(dci: u8) -> *mut Trb {
    phys_to_virt(dma_phys() + endpoint_ring_off(dci)) as *mut Trb
}

#[inline]
fn ctrl_data_virt() -> *mut u8 {
    phys_to_virt(dma_phys() + CTRL_DATA_OFF) as *mut u8
}

#[inline]
fn int_data_virt() -> *mut u8 {
    phys_to_virt(dma_phys() + INT_DATA_OFF) as *mut u8
}

#[inline]
fn input_ctx_virt() -> *mut u32 {
    phys_to_virt(dma_phys() + INPUT_CTX_OFF) as *mut u32
}

#[inline]
fn output_ctx_phys(slot: u8) -> usize {
    let idx = (slot as usize).saturating_sub(1);
    dma_phys() + OUTPUT_CTX_OFF + idx * OUTPUT_CONTEXT_SIZE
}

#[inline]
fn output_ctx_virt(slot: u8) -> *mut u32 {
    phys_to_virt(output_ctx_phys(slot)) as *mut u32
}

#[inline]
fn dcbaa_virt() -> *mut u64 {
    phys_to_virt(dma_phys() + DCBAA_OFF) as *mut u64
}

// ─── Supported Protocol extended capability parsing ────────────────────
//
// Real xHCI controllers expose USB2 and USB3 root ports interleaved in the
// port register set. The only authoritative source of "logical port N is
// USB2 or USB3, and its PORTSC lives at this MMIO index" is the Supported
// Protocol extended capability (xHCI spec 7.2). We walk the extended
// capability list pointed at by HCCPARAMS1.xECP and, for every ID=2 entry,
// record `port_offset..port_offset+port_count` (1-based, inclusive) into
// `PORT_MAJOR_REV` / `PORT_MMIO_IDX`.
//
// The MMIO port register set is contiguous: port register set #i lives at
// opbase + 0x400 + i*0x10, with i in 0..MaxPorts. Supported Protocol entries
// declare a *logical* port range (1-based) that maps 1:1 to those register
// sets in order, so logical port P corresponds to MMIO index P-1. We still
// store the index explicitly so the mapping is self-documenting and so a
// future controller with a non-contiguous declaration can be handled without
// touching every caller.

/// Claim xHCI ownership from firmware via the USB Legacy Support extended
/// capability. Without this handoff, firmware SMI handlers may continue to
/// manage root-port power and keyboard emulation behind the OS, which is
/// exactly the kind of real-hardware failure where enumeration partly works but
/// devices lose power or never generate runtime input events.
fn take_bios_ownership() {
    if XHCI_MMIO.load(Ordering::Relaxed) == 0 {
        return;
    }

    let hcc = unsafe { reg_read(CAP_HCCPARAMS1) };
    let xecp_dwords = ((hcc >> 16) & 0xFFFF) as usize;
    if xecp_dwords == 0 {
        return;
    }

    let mut offset = xecp_dwords * 4;
    let mut guard = 0u32;
    while offset != 0 && guard < 256 {
        guard += 1;
        let header = unsafe { reg_read(offset) };
        if header == 0xFFFF_FFFF {
            break;
        }
        let id = header & 0xFF;
        let next = ((header >> 8) & 0xFF) as usize;

        if id == XEC_ID_LEGACY {
            let mut legsup = header;
            if legsup & XHCI_LEGACY_BIOS_OWNED != 0 {
                unsafe {
                    reg_write(offset, legsup | XHCI_LEGACY_OS_OWNED);
                }
                for _ in 0..1000 {
                    legsup = unsafe { reg_read(offset) };
                    if legsup & XHCI_LEGACY_BIOS_OWNED == 0 {
                        break;
                    }
                    wait_ms(1);
                }
            } else if legsup & XHCI_LEGACY_OS_OWNED == 0 {
                unsafe {
                    reg_write(offset, legsup | XHCI_LEGACY_OS_OWNED);
                }
            }
            // USBLEGCTLSTS follows USBLEGSUP. Clear SMI enables after ownership
            // handoff; status bits are harmless if already zero.
            unsafe {
                reg_write(offset + 4, 0);
            }
        }

        offset = if next == 0 { 0 } else { offset + next * 4 };
    }
}

/// Walk the xHCI extended capability list and populate per-logical-port
/// protocol info. Safe to call once during init after `XHCI_MMIO` is set and
/// before any port access. Reads only capability registers.
fn parse_supported_protocols() {
    if XHCI_MMIO.load(Ordering::Relaxed) == 0 {
        return;
    }
    let max_ports = XHCI_MAX_PORTS.load(Ordering::Relaxed) as usize;

    let hcc = unsafe { reg_read(CAP_HCCPARAMS1) };
    let xecp_dwords = ((hcc >> 16) & 0xFFFF) as usize;
    if xecp_dwords == 0 {
        crate::console_println!("[xhci] no extended capabilities");
        return;
    }

    // Reset per-port tables to the default (USB2, idx=port-1) so re-init does
    // not accumulate stale entries. We write 0 first, then callers fall back
    // via port_mmio_index(); after parsing we set explicit values.
    for p in 1..=max_ports.min(PORT_CAPS_LEN - 1) {
        PORT_MAJOR_REV[p].store(0, Ordering::Relaxed);
        PORT_MMIO_IDX[p].store(0, Ordering::Relaxed);
    }

    let mut offset = xecp_dwords * 4; // xECP is in dwords; convert to bytes.
    let mut found_protocols = 0u32;
    let mut usb2_count = 0u32;
    let mut usb3_count = 0u32;
    let mut guard = 0u32;
    while offset != 0 && guard < 256 {
        guard += 1;
        let header = unsafe { reg_read(offset) };
        if header == 0xFFFF_FFFF {
            break;
        }
        let id = header & 0xFF;
        let next = ((header >> 8) & 0xFF) as usize;

        if id == XEC_ID_PROTOCOL {
            // dword 0: bits 24:31 major rev, 16:23 minor rev.
            let major = ((header >> 24) & 0xFF) as u8;
            // dword 2 (byte offset +8): bits 0:7 port_offset (1-based),
            //   bits 8:15 port_count.
            let port_info = unsafe { reg_read(offset + 8) };
            let port_offset = (port_info & 0xFF) as usize;
            let port_count = ((port_info >> 8) & 0xFF) as usize;
            found_protocols += 1;
            if major == 3 {
                usb3_count += 1;
            } else if major <= 2 {
                usb2_count += 1;
            }
            if port_offset == 0 || port_count == 0 {
                offset = if next == 0 { 0 } else { offset + next * 4 };
                continue;
            }
            // Logical port numbers are 1-based and inclusive. The MMIO port
            // register set index is logical_port - 1 (spec 5.4.8: port
            // register set N corresponds to logical port N+1... but the
            // convention used by Linux and real hardware is that the port
            // register set array is indexed 0..MaxPorts-1 in the same order
            // the Supported Protocol capabilities declare logical ports, so
            // logical port P maps to MMIO index P-1).
            for i in 0..port_count {
                let logical = port_offset + i; // 1-based
                if logical == 0 || logical >= PORT_CAPS_LEN {
                    continue;
                }
                if logical > max_ports {
                    break;
                }
                PORT_MAJOR_REV[logical].store(major, Ordering::Relaxed);
                PORT_MMIO_IDX[logical].store((logical - 1) as u8, Ordering::Relaxed);
            }
        }

        offset = if next == 0 { 0 } else { offset + next * 4 };
    }

    crate::console_println!(
        "[xhci] protocols: entries={} usb2={} usb3={}",
        found_protocols,
        usb2_count,
        usb3_count
    );
}

// ─── Controller initialization ─────────────────────────────────────────

/// Probe and initialize the XHCI controller. Safe: does not enumerate devices.
pub fn init() -> Result<(), &'static str> {
    let dev = crate::arch::pci::find_xhci().ok_or("XHCI not found")?;
    init_device(dev)
}

/// Initialize every XHCI controller, enumerate each one, and keep the last
/// controller that exposes a keyboard candidate active.
///
/// Real desktops often have several independent xHCs. On the current test
/// machine the front-panel devices are on bus 001, while the Rapoo keyboard is
/// on a different xHC (Linux shows it as bus 007). A single `find_xhci()` call
/// binds only the first controller and can stop at a mouse's composite
/// "keyboard" interface. We scan all xHCs, remember the last controller with a
/// keyboard candidate, then re-initialize that controller so the global single-
/// controller state (MMIO, rings, IRQ route) points at the selected keyboard.
pub fn init_all_and_enumerate() -> Result<(), &'static str> {
    let controllers = crate::arch::pci::find_xhci_all();
    if controllers.is_empty() {
        return Err("XHCI not found");
    }

    crate::console_println!("[xhci] scanning {} controllers", controllers.len());
    let mut selected: Option<crate::arch::pci::PciDevice> = None;
    let mut any_ok = false;
    for dev in controllers {
        clear_keyboard_state();
        match init_device(dev) {
            Ok(()) => {
                any_ok = true;
                if enumerate_devices_scan() {
                    crate::console_println!(
                        "[xhci] controller {:02x}:{:02x}.{} has keyboard candidate",
                        dev.bus,
                        dev.device,
                        dev.function
                    );
                    selected = Some(dev);
                }
            }
            Err(e) => {
                crate::console_println!(
                    "[xhci] controller {:02x}:{:02x}.{} init failed: {}",
                    dev.bus,
                    dev.device,
                    dev.function,
                    e
                );
            }
        }
    }

    if let Some(dev) = selected {
        crate::console_println!(
            "[xhci] selecting controller {:02x}:{:02x}.{}",
            dev.bus,
            dev.device,
            dev.function
        );
        clear_keyboard_state();
        init_device(dev)?;
        let _ = enumerate_devices();
        Ok(())
    } else if any_ok {
        Ok(())
    } else {
        Err("all XHCI init failed")
    }
}

fn init_device(dev: crate::arch::pci::PciDevice) -> Result<(), &'static str> {
    XHCI_READY.store(false, Ordering::Relaxed);
    reset_ring_state();
    KBD_XFER_PENDING.store(false, Ordering::Release);
    SYNC_ENUM_ACTIVE.store(false, Ordering::Release);
    crate::console_println!(
        "[xhci] Found at {:02x}:{:02x}.{}",
        dev.bus,
        dev.device,
        dev.function
    );

    dev.enable();
    let bar0 = dev.bar_address(0) as usize;
    if bar0 == 0 {
        return Err("XHCI BAR0 is zero");
    }
    XHCI_MMIO.store(bar0 as u64, Ordering::Relaxed);

    unsafe {
        let cap_len = core::ptr::read_volatile(bar0 as *const u8) as usize;
        XHCI_OPBASE.store((bar0 + cap_len) as u64, Ordering::Relaxed);
        XHCI_DBBASE.store(
            (bar0 + (reg_read(CAP_DBOFF) & !3) as usize) as u64,
            Ordering::Relaxed,
        );
        XHCI_RTBASE.store(
            (bar0 + (reg_read(CAP_RTSOFF) & !3) as usize) as u64,
            Ordering::Relaxed,
        );

        let hcs_params1 = reg_read(CAP_HCSPARAMS1);
        let hcc_params1 = reg_read(CAP_HCCPARAMS1);
        let ports = ((hcs_params1 >> 24) & 0xFF) as u64;
        let hw_slots = (hcs_params1 & 0xFF) as u64;
        let slots = hw_slots.min(MAX_SUPPORTED_SLOTS as u64);
        let ctx_size = if hcc_params1 & (1 << 2) != 0 { 64 } else { 32 };
        XHCI_MAX_PORTS.store(ports, Ordering::Relaxed);
        XHCI_MAX_SLOTS.store(slots, Ordering::Relaxed);
        XHCI_CONTEXT_SIZE.store(ctx_size, Ordering::Relaxed);
        crate::console_println!(
            "[xhci] cap_len={} ports={} slots={} ctx_size={}",
            cap_len,
            ports,
            slots,
            ctx_size
        );

        // Take ownership before reset/port-power operations. Firmware may keep
        // legacy keyboard SMI handlers active until OS Owned is set.
        take_bios_ownership();

        // Parse the Supported Protocol extended capabilities before any port
        // access so port_read/port_write resolve the correct MMIO indices and
        // port_reset picks the protocol-appropriate reset sequence. This runs
        // before the controller reset below because extended capabilities live
        // in the read-only capability register space and are unaffected by it.
        parse_supported_protocols();

        // Stop and reset the controller.
        let usbcmd = op_read(OP_USBCMD);
        op_write(OP_USBCMD, usbcmd & !CMD_RUN);
        for _ in 0..50000 {
            if op_read(OP_USBSTS) & STS_HCH != 0 {
                break;
            }
        }
        op_write(OP_USBCMD, CMD_HCRST);
        for _ in 0..200000 {
            if op_read(OP_USBSTS) & STS_CNR == 0 {
                break;
            }
        }
        if op_read(OP_USBSTS) & STS_CNR != 0 {
            return Err("reset timeout");
        }
        crate::console_println!("[xhci] Reset done");

        op_write(OP_CONFIG, slots as u32);

        // Allocate the DMA arena.
        let buf_phys =
            crate::mm::pmm::alloc_contiguous_frames(DMA_PAGES).ok_or("DMA alloc failed")?;
        DMA_PHYS.store(buf_phys as u64, Ordering::Relaxed);
        let buf_virt = phys_to_virt(buf_phys);
        core::ptr::write_bytes(buf_virt as *mut u8, 0, DMA_PAGES * 4096);

        // Set up DCBAA. Entry 0 is reserved (null); entries 1..=slots point
        // to per-slot output contexts. For now all entries are zero; they get
        // filled in when a slot is enabled.
        op_write(OP_DCBAAP_LO, (buf_phys + DCBAA_OFF) as u32);
        op_write(OP_DCBAAP_HI, ((buf_phys + DCBAA_OFF) >> 32) as u32);

        // Command Ring: write a Link TRB at the last slot pointing back to the
        // ring start, with the cycle bit matching the producer state.
        let cr_phys = (buf_phys + CMD_RING_OFF) as u64;
        let cr_ring = cmd_ring_virt();
        // Link TRB at index RING_SIZE-1
        let link_trb = Trb {
            parameter: cr_phys,
            status: 0,
            control: Trb::control(TRB_LINK, TRB_CYCLE_BIT), // cycle=1 matches initial producer cycle
        };
        cr_ring.add(RING_SIZE - 1).write_volatile(link_trb);
        // Program CRCR with the ring base + RCS=1
        op_write(OP_CRCR_LO, cr_phys as u32 | 1);
        op_write(OP_CRCR_HI, (cr_phys >> 32) as u32);

        // Control transfer ring (EP0): write a Link TRB at the last slot so the
        // ring wraps correctly. The cycle bit starts matching the producer cycle
        // (true), and the enqueue logic toggles it when it reaches the link.
        let ctrl_phys = (buf_phys + CTRL_RING_OFF) as u64;
        let ctrl_ring = ctrl_ring_virt();
        ctrl_ring.add(RING_SIZE - 1).write_volatile(Trb {
            parameter: ctrl_phys,
            status: 0,
            control: Trb::control(TRB_LINK, TRB_CYCLE_BIT),
        });

        // Interrupt transfer ring (HID keyboard IN endpoint): same Link TRB.
        let int_phys = (buf_phys + INT_RING_OFF) as u64;
        let int_ring = int_ring_virt();
        int_ring.add(RING_SIZE - 1).write_volatile(Trb {
            parameter: int_phys,
            status: 0,
            control: Trb::control(TRB_LINK, TRB_CYCLE_BIT),
        });

        // Additional interrupt endpoint rings, indexed by DCI. Composite HID
        // devices such as keyboards with an extra mouse interface have multiple
        // active periodic endpoints in one configuration; each endpoint needs a
        // distinct transfer ring even if only the keyboard ring is primed.
        for dci in 1..=MAX_ENDPOINTS as u8 {
            let ring_phys = (buf_phys + endpoint_ring_off(dci)) as u64;
            endpoint_ring_virt(dci)
                .add(RING_SIZE - 1)
                .write_volatile(Trb {
                    parameter: ring_phys,
                    status: 0,
                    control: Trb::control(TRB_LINK, TRB_CYCLE_BIT),
                });
        }

        // Event Ring Segment Table (ERST): one segment of RING_SIZE TRBs.
        let erst_phys = (buf_phys + ERST_OFF) as u64;
        let erst = phys_to_virt(erst_phys as usize) as *mut ErstEntry;
        let evt_seg_phys = (buf_phys + EVENT_RING_OFF) as u64;
        erst.write_volatile(ErstEntry {
            ring_seg_addr_lo: evt_seg_phys as u32,
            ring_seg_addr_hi: (evt_seg_phys >> 32) as u32,
            ring_seg_size: RING_SIZE as u32,
            reserved: 0,
        });

        // Program Interrupter 0 ERST size/base and ERDP.
        rt_write(RT_IR0_ERSTSZ, 1);
        rt_write(RT_IR0_ERSTBA_LO, erst_phys as u32);
        rt_write(RT_IR0_ERSTBA_HI, (erst_phys >> 32) as u32);
        // ERDP = event ring base, with EHB=0 (no events consumed yet).
        rt_write(RT_IR0_ERDP_LO, evt_seg_phys as u32);
        rt_write(RT_IR0_ERDP_HI, (evt_seg_phys >> 32) as u32);

        // Some xHCs accept PP writes while halted; others only latch them once
        // the controller is running. Do both and log the readback.
        power_on_all_ports(ports as usize, "pre-run");

        // Start the controller with interrupt delivery enabled. Enumeration
        // still polls command/control completions synchronously, while HID
        // interrupt endpoint completions are consumed by `handle_irq`.
        op_write(OP_USBCMD, CMD_RUN | CMD_INTE | CMD_HSEE);
        for _ in 0..10000 {
            if op_read(OP_USBSTS) & STS_HCH == 0 {
                break;
            }
        }

        crate::console_println!("[xhci] Running");
        power_on_all_ports(ports as usize, "post-run");
    }

    // Route the XHCI interrupt. Install the IDT handler at IRQ_BASE+irq_line
    // and program the IOAPIC to deliver that pin to the BSP. We do this after
    // the controller is running so a stray IRQ before init completes cannot
    // fire into an uninstalled vector.
    let irq_line = dev.irq_line;
    if irq_line > 0 && irq_line < 16 {
        crate::arch::idt::install_xhci_vector(irq_line);
        crate::arch::ioapic::route_pci_intx(irq_line, crate::arch::idt::IRQ_BASE + irq_line);
    } else {
        crate::console_println!("[xhci] no valid irq_line ({}); polling only", irq_line);
    }

    XHCI_READY.store(true, Ordering::Relaxed);
    Ok(())
}

pub fn is_available() -> bool {
    XHCI_READY.load(Ordering::Relaxed)
}

fn reset_ring_state() {
    let mut state = RING_LOCK.lock();
    *state = RingState {
        cmd_cycle: true,
        cmd_index: 0,
        evt_cycle: true,
        evt_index: 0,
        ctrl_cycle: true,
        ctrl_index: 0,
        int_cycle: true,
        int_index: 0,
    };
}

fn clear_keyboard_state() {
    HAS_KEYBOARD.store(false, Ordering::Release);
    KEYBOARD_SLOT.store(0, Ordering::Release);
    KEYBOARD_EP_DCI.store(0, Ordering::Release);
    KEYBOARD_IFACE.store(0, Ordering::Release);
    KEYBOARD_PROTO.store(0, Ordering::Release);
    KEYBOARD_REPORT_LEN.store(8, Ordering::Release);
    KBD_XFER_PENDING.store(false, Ordering::Release);
    KBD_GET_REPORT_DISABLED.store(false, Ordering::Release);
}

// ─── Command submission ────────────────────────────────────────────────

/// Enqueue a TRB on the command ring and ring the controller doorbell.
/// The caller must hold the RING_LOCK.
unsafe fn cmd_enqueue_locked(state: &mut RingState, trb: Trb, cycle: bool) {
    let ring = cmd_ring_virt();
    let mut t = trb;
    if cycle {
        t.control |= TRB_CYCLE_BIT;
    } else {
        t.control &= !TRB_CYCLE_BIT;
    }
    // If we are about to write the Link TRB slot, advance through it.
    if state.cmd_index == RING_SIZE - 1 {
        // Toggle the link TRB cycle and follow it.
        let link = ring.add(RING_SIZE - 1).read_volatile();
        let mut new_link = link;
        if cycle {
            new_link.control |= TRB_CYCLE_BIT;
        } else {
            new_link.control &= !TRB_CYCLE_BIT;
        }
        ring.add(RING_SIZE - 1).write_volatile(new_link);
        state.cmd_index = 0;
        state.cmd_cycle = !state.cmd_cycle;
    }
    ring.add(state.cmd_index).write_volatile(t);
    state.cmd_index += 1;
    doorbell_ring(0, 0);
}

/// Wait for a Command Completion Event on the event ring. Returns the
/// completion code and slot id from the event TRB, or None on timeout.
///
/// This polls the event ring because the first commands run before interrupts
/// are routed. Once IRQ is wired, `handle_irq` will advance the event ring
/// and wake any waiter; for now polling is acceptable during enumeration.
unsafe fn wait_cmd_complete() -> Option<(u32, u8)> {
    let evt = event_ring_virt();
    let start = read_tsc();
    let timeout_cycles = CMD_TIMEOUT_MS.saturating_mul(TSC_CYCLES_PER_MS);
    while read_tsc().wrapping_sub(start) < timeout_cycles {
        let state = RING_LOCK.lock();
        let idx = state.evt_index;
        let evt_cycle = state.evt_cycle;
        drop(state);
        let e = evt.add(idx).read_volatile();
        let producer_cycle = e.cycle();
        // The event is valid when its cycle bit equals the consumer's cycle.
        if producer_cycle == evt_cycle {
            let typ = e.trb_type();
            if typ == TRB_CMD_COMPLETE {
                let cc = e.completion_code();
                let slot = e.slot_id();
                // Advance the event ring consumer.
                advance_event_ring();
                return Some((cc, slot));
            }
            // Other event types (port status change) — skip for now.
            advance_event_ring();
        }
        core::hint::spin_loop();
    }
    dump_event_wait_timeout("cmd");
    None
}

/// Advance the event ring consumer pointer by one TRB, toggling cycle when
/// wrapping, and update ERDP so the controller knows we consumed it.
unsafe fn advance_event_ring() {
    let mut state = RING_LOCK.lock();
    state.evt_index += 1;
    if state.evt_index >= RING_SIZE {
        state.evt_index = 0;
        state.evt_cycle = !state.evt_cycle;
    }
    drop(state);
    let evt_phys = (dma_phys() + EVENT_RING_OFF) as u64;
    let new_erdp = evt_phys + (RING_LOCK.lock().evt_index as u64) * 16;
    // EHB bit (bit 3) must be preserved/set to acknowledge the event.
    rt_write(RT_IR0_ERDP_LO, new_erdp as u32 | (1 << 3));
    rt_write(RT_IR0_ERDP_HI, (new_erdp >> 32) as u32);
}

/// Submit a command TRB and wait synchronously for its completion.
pub fn submit_command(trb: Trb) -> Option<(u32, u8)> {
    if !is_available() {
        return None;
    }
    unsafe {
        let mut state = RING_LOCK.lock();
        let cycle = state.cmd_cycle;
        cmd_enqueue_locked(&mut state, trb, cycle);
        drop(state);
        wait_cmd_complete()
    }
}

// ─── Port helpers ──────────────────────────────────────────────────────

pub fn port_is_connected(port: usize) -> bool {
    if !is_available() {
        return false;
    }
    unsafe { port_read(port) & PORT_CCS != 0 }
}

unsafe fn power_on_all_ports(ports: usize, _tag: &str) {
    for p in 1..=ports {
        let sc = port_read(p);
        if sc & PORT_PP == 0 {
            port_write_preserve(p, sc | PORT_PP, 0);
            wait_ms(20);
        }
    }
}

/// Reset a port and return the connected device's speed.
///
/// USB2 and USB3 root ports use different reset semantics (xHCI spec 5.4.8):
///   - USB2 ports: write PORT_PR (bit 4); the controller drives the SE0/reset
///     condition, then sets PED and clears PR. We wait for PR=0 && PED=1.
///   - USB3 ports: write PORT_WPR (bit 31) for a warm reset, or rely on the
///     link's automatic Polling->U0 training on connect. We wait for the link
///     to reach U0 (PLS=0) with PED=1, and fall back to a warm reset if the
///     link does not come up on its own within a short window.
pub fn port_reset(port: usize) -> Option<UsbSpeed> {
    if !is_available() {
        return None;
    }
    let major = port_major_rev(port);
    let is_usb3 = major == 3;
    unsafe {
        let mut sc = port_read(port);
        if sc & PORT_CCS == 0 {
            return None;
        }

        // Acknowledge stale change bits before starting a fresh reset; real
        // controllers commonly leave CSC/PLC/PRC set after firmware handoff.
        let stale_changes = sc & PORT_CHANGE_MASK;
        if stale_changes != 0 {
            port_write_preserve(port, sc, stale_changes);
            wait_ms(1);
            sc = port_read(port);
        }

        // USB3 ports often train to U0 automatically on connect. Give them a
        // short window first; only force a reset if the link is stuck. USB2
        // ports always require an explicit reset.
        if is_usb3 {
            // Wait up to 50ms for autonomous U0.
            let auto_start = read_tsc();
            let auto_to = 50u64.saturating_mul(TSC_CYCLES_PER_MS);
            while read_tsc().wrapping_sub(auto_start) < auto_to {
                let s = port_read(port);
                let pls = (s & PORT_PLS_MASK) >> PORT_PLS_SHIFT;
                if s & PORT_CCS != 0 && s & PORT_PED != 0 && pls == PLS_U0 {
                    let speed_code = (s & PORT_SPEED_MASK) >> PORT_SPEED_SHIFT;
                    return Some(UsbSpeed::from_xhci_code(speed_code));
                }
                wait_ms(1);
            }
            // Link did not auto-train; issue a warm reset.
            port_write_preserve(port, sc | PORT_WPR, 0);
        } else {
            // USB2 reset.
            port_write_preserve(port, sc | PORT_PR, 0);
        }

        // Wait for reset to complete. For USB2 we look for PR=0 && PED=1.
        // For USB3 we accept either PED=1 (enabled) or PLS=U0 with the warm
        // reset change (WRC) cleared, whichever the controller reports first.
        let reset_bit = if is_usb3 { PORT_WPR } else { PORT_PR };
        for _ in 0..500 {
            let s = port_read(port);
            if s & reset_bit == 0 && s & PORT_PED != 0 {
                let speed_code = (s & PORT_SPEED_MASK) >> PORT_SPEED_SHIFT;
                return Some(UsbSpeed::from_xhci_code(speed_code));
            }
            wait_ms(1);
        }
        let final_sc = port_read(port);
        let pls = (final_sc & PORT_PLS_MASK) >> PORT_PLS_SHIFT;
        let speed = (final_sc & PORT_SPEED_MASK) >> PORT_SPEED_SHIFT;
        crate::console_println!(
            "[xhci] port {} reset timeout PORTSC={:#x} CCS={} PED={} {}={} PLS={} speed={} usb3={}",
            port,
            final_sc,
            if final_sc & PORT_CCS != 0 { 1 } else { 0 },
            if final_sc & PORT_PED != 0 { 1 } else { 0 },
            if is_usb3 { "WPR" } else { "PR" },
            if final_sc & reset_bit != 0 { 1 } else { 0 },
            pls,
            speed,
            if is_usb3 { 1 } else { 0 }
        );
        None
    }
}

/// Clear the port reset change bit (write-1-to-clear).
pub fn port_clear_reset_change(port: usize) {
    if !is_available() {
        return;
    }
    unsafe {
        let sc = port_read(port);
        port_write_preserve(port, sc, PORT_PRC);
    }
}

/// Clear all W1C root-port change bits that are currently set and record the
/// changed port in `ROOT_PORT_CHANGE_BITS`. This is intentionally lightweight:
/// it is safe for the XHCI ISR and never runs USB enumeration or transfers.
unsafe fn handle_root_hub_port_changes() {
    let ports = XHCI_MAX_PORTS.load(Ordering::Relaxed) as usize;
    for port in 1..=ports {
        let sc = port_read(port);
        let changes = sc & PORT_CHANGE_MASK;
        if changes == 0 {
            continue;
        }

        ROOT_PORT_CHANGE_BITS.fetch_or(1u64 << (port - 1), Ordering::AcqRel);

        // W1C: preserve current status/control bits, write ones to change bits.
        port_write_preserve(port, sc, changes);

        // If the known HID keyboard disappeared, stop re-priming its endpoint.
        if sc & PORT_CCS == 0 && HAS_KEYBOARD.load(Ordering::Relaxed) {
            HAS_KEYBOARD.store(false, Ordering::Release);
            KEYBOARD_SLOT.store(0, Ordering::Release);
            KEYBOARD_EP_DCI.store(0, Ordering::Release);
            KBD_XFER_PENDING.store(false, Ordering::Release);
        }
    }
}

/// Return and clear the pending root-hub port change bitmap. Bit N maps to
/// port N+1. A future non-ISR hub worker can use this to trigger deferred
/// attach/detach enumeration.
pub fn take_root_port_changes() -> u64 {
    ROOT_PORT_CHANGE_BITS.swap(0, Ordering::AcqRel)
}

// ─── Device context helpers ────────────────────────────────────────────

/// Set the DCBAA entry for a slot to point at its output context.
unsafe fn set_slot_dcbaa(slot: u8) {
    let dcbaa = dcbaa_virt();
    if slot == 0 || slot as usize > MAX_SUPPORTED_SLOTS {
        return;
    }
    dcbaa
        .add(slot as usize)
        .write_volatile(output_ctx_phys(slot) as u64);
}

/// Build the Input Context for an Address Device command.
///
/// Layout (CSZ=0, 32-byte contexts):
///   bytes 0..31   = Input Control Context (dword 0 = A0 drop flags,
///                   dword 1 = A1 add flags, dwords 2..7 reserved)
///   bytes 32..63  = Slot Context
///   bytes 64..95  = Endpoint Context for DCI 1 (EP0)
///   bytes 96..    = Endpoint Contexts for DCI 2..31
unsafe fn build_input_context_address(
    port: u8,
    speed: UsbSpeed,
    max_packet_size0: u16,
    ictx: *mut u32,
) {
    // Zero the entire input context first, then write the fields we need.
    core::ptr::write_bytes(ictx as *mut u8, 0, INPUT_CONTEXT_SIZE);

    // Input Control Context: A0 = drop flags, A1 = add flags.
    // Add slot context (bit 0) and control endpoint DCI=1 (bit 1).
    ictx.add(ICTX_A0).write_volatile(0); // drop context: none
    ictx.add(ICTX_A1).write_volatile(0x3); // add context: slot (bit0) + EP0 (bit1)

    // Slot Context starts after the controller-sized Input Control Context.
    let slot_base = ictx.add(slot_ctx_dword());
    // dword 0 (dev_info): route string (0-19)=0, speed (20-23)=hw code,
    //   MTT (25)=0, hub (26)=0, context entries (27-31)=1 (EP0 DCI).
    let speed_code = speed.xhci_code();
    let dev_info: u32 = (speed_code << 20) | (1 << 27);
    slot_base.add(SLOT_DEV_INFO).write_volatile(dev_info);
    // dword 1 (dev_info2): max exit latency (0-15)=0, root hub port (16-23).
    slot_base
        .add(SLOT_DEV_INFO2)
        .write_volatile((port as u32) << 16);
    // dword 2 (tt_info): interrupter target (0-15)=0 (use primary interrupter).
    slot_base.add(SLOT_TT_INFO).write_volatile(0);
    // dword 3 (dev_state): slot state set by HC; leave 0 for input context.

    // Endpoint Context for EP0 (DCI=1), starting at dword ep_ctx_dword(1) = 16.
    let ep0 = ictx.add(ep_ctx_dword(1));
    // dword 0 (ep_info): EP state=0(disabled, input), interval=0 for control.
    ep0.add(EP_INFO).write_volatile(0);
    // dword 1 (ep_info2): CErr(1-2)=3, EP type(3-5)=control(4), max packet(16-31).
    let ep_info2: u32 =
        (3u32 << 1) | (EP_TYPE_CONTROL_OUT << 3) | ((max_packet_size0 as u32) << 16);
    ep0.add(EP_INFO2).write_volatile(ep_info2);
    // dword 2..3 (deq): TR dequeue pointer = control ring base, DCS=1 (bit0).
    let ctrl_ring_phys = (dma_phys() + CTRL_RING_OFF) as u64;
    ep0.add(EP_DEQ_LO).write_volatile(ctrl_ring_phys as u32 | 1);
    ep0.add(EP_DEQ_HI)
        .write_volatile((ctrl_ring_phys >> 32) as u32);
    // dword 4 (tx_info): avg TRB length = 8 for control transfers.
    ep0.add(EP_TX_INFO).write_volatile(8);
}

// ─── Control transfer ──────────────────────────────────────────────────

/// Perform a USB control transfer (setup + optional data + status) on EP0
/// of the given slot. Returns the number of bytes transferred on success.
pub fn control_transfer(
    slot: u8,
    setup: SetupPacket,
    buf: Option<(&mut [u8], bool)>, // (buffer, direction_in)
) -> Result<usize, &'static str> {
    if !is_available() {
        return Err("xhci not ready");
    }
    unsafe {
        let data_phys = (dma_phys() + CTRL_DATA_OFF) as u64;
        let data_virt = ctrl_data_virt();
        let length = setup.w_length as usize;

        // Copy OUT data into the DMA buffer.
        if let Some((b, dir_in)) = &buf {
            if !dir_in && length > 0 {
                core::ptr::copy_nonoverlapping(b.as_ptr(), data_virt, length.min(b.len()));
            }
        }

        // Enqueue Setup, Data, Status TRBs on the control ring.
        let mut state = RING_LOCK.lock();
        let cycle = state.ctrl_cycle;

        // Determine data stage direction and Transfer Type (TRT) for the
        // Setup TRB. TRT tells the controller whether a data stage exists
        // and its direction.
        let dir_in = buf.as_ref().map(|(_, d)| *d).unwrap_or(false);
        let has_data = length > 0;
        let trt: u32 = if !has_data {
            TRB_TRT_NONE
        } else if dir_in {
            TRB_TRT_IN
        } else {
            TRB_TRT_OUT
        };

        // Setup stage TRB. IDT (bit 6) marks the 8-byte setup packet as
        // carried inline in the parameter field (not a DMA pointer).
        // No IOC on the setup stage; the status stage will fire the event.
        let setup_trb = Trb {
            parameter: setup.encode_trb_parameter(),
            status: 8,
            control: Trb::control(TRB_SETUP_STAGE, trt | TRB_IDT),
        };
        enqueue_ctrl_locked(&mut state, setup_trb, cycle);

        // Data stage (if any). Single TRB, no chain. Direction in bit 16.
        if has_data {
            let data_dir_bit: u32 = if dir_in { TRB_DIR_IN } else { TRB_DIR_OUT };
            let data_trb = Trb {
                parameter: data_phys,
                status: length as u32,
                control: Trb::control(TRB_DATA_STAGE, data_dir_bit),
            };
            enqueue_ctrl_locked(&mut state, data_trb, cycle);
        }

        // Status stage TRB. Direction is opposite of data (IN data -> OUT
        // status; no data or OUT data -> IN status). IOC fires the event.
        let status_dir: u32 = if has_data && dir_in {
            TRB_DIR_OUT
        } else {
            TRB_DIR_IN
        };
        let status_trb = Trb {
            parameter: 0,
            status: 0,
            control: Trb::control(TRB_STATUS_STAGE, status_dir | TRB_IOC),
        };
        let status_idx = if state.ctrl_index == RING_SIZE - 1 {
            0
        } else {
            state.ctrl_index
        };
        let status_trb_phys =
            (dma_phys() + CTRL_RING_OFF + status_idx * core::mem::size_of::<Trb>()) as u64;
        enqueue_ctrl_locked(&mut state, status_trb, cycle);
        drop(state);

        // Ring doorbell for EP0 (DCI=1)
        doorbell_ring(slot as u32, 1);

        // Wait for the Transfer Event generated by this transfer's Status TRB.
        let (cc, _ep) = match wait_transfer_event(slot, 1, status_trb_phys) {
            Some(v) => v,
            None => return Err("transfer event timeout"),
        };
        if cc != TRB_CC_SUCCESS {
            return Err("control transfer failed");
        }

        // Copy IN data back to the user buffer.
        if let Some((b, dir_in)) = buf {
            if dir_in && length > 0 {
                let n = length.min(b.len());
                core::ptr::copy_nonoverlapping(data_virt, b.as_mut_ptr(), n);
                return Ok(n);
            }
        }
        Ok(length)
    }
}

unsafe fn enqueue_ctrl_locked(state: &mut RingState, trb: Trb, cycle: bool) {
    let ring = ctrl_ring_virt();
    let mut t = trb;
    if cycle {
        t.control |= TRB_CYCLE_BIT;
    } else {
        t.control &= !TRB_CYCLE_BIT;
    }
    if state.ctrl_index == RING_SIZE - 1 {
        let link = ring.add(RING_SIZE - 1).read_volatile();
        let mut new_link = link;
        new_link.control =
            (new_link.control & !TRB_CYCLE_BIT) | if cycle { TRB_CYCLE_BIT } else { 0 };
        ring.add(RING_SIZE - 1).write_volatile(new_link);
        state.ctrl_index = 0;
        state.ctrl_cycle = !state.ctrl_cycle;
    }
    ring.add(state.ctrl_index).write_volatile(t);
    state.ctrl_index += 1;
}

/// Wait for the Transfer Event generated by a specific control-transfer Status
/// TRB. Event rings are shared by all slots/endpoints; accepting "any EP0"
/// event corrupts enumeration once multiple devices are present, because an
/// old slot's EP0 completion can be mistaken for the current slot's 8-byte
/// descriptor read. Match slot id, endpoint id, and TRB pointer.
unsafe fn wait_transfer_event(
    slot: u8,
    expected_ep: u8,
    expected_trb_phys: u64,
) -> Option<(u32, u8)> {
    let evt = event_ring_virt();
    let start = read_tsc();
    let timeout_cycles = CONTROL_TRANSFER_TIMEOUT_MS.saturating_mul(TSC_CYCLES_PER_MS);
    while read_tsc().wrapping_sub(start) < timeout_cycles {
        let state = RING_LOCK.lock();
        let idx = state.evt_index;
        let evt_cycle = state.evt_cycle;
        drop(state);
        let e = evt.add(idx).read_volatile();
        if e.cycle() == evt_cycle {
            let typ = e.trb_type();
            if typ == TRB_TRANSFER_EVENT {
                let cc = e.completion_code();
                let ep = e.endpoint_id();
                let evt_slot = e.slot_id();
                let evt_trb = e.parameter & !0xFu64;
                if evt_slot != slot || ep != expected_ep {
                    process_event(e);
                    advance_event_ring();
                    continue;
                }
                if evt_trb != expected_trb_phys {
                    // This is a real event for the current control endpoint,
                    // but not the Status TRB that completes this transfer.
                    // Preserve errors; otherwise consume the intermediate event
                    // and wait for the status-stage completion.
                    if cc != TRB_CC_SUCCESS && cc != TRB_CC_SHORT_PACKET {
                        advance_event_ring();
                        return Some((cc, ep));
                    }
                    advance_event_ring();
                    continue;
                }
                advance_event_ring();
                return Some((cc, ep));
            }
            // Skip non-transfer events.
            advance_event_ring();
        }
        core::hint::spin_loop();
    }
    dump_event_wait_timeout("transfer");
    None
}

unsafe fn dump_event_wait_timeout(kind: &str) {
    let state = RING_LOCK.lock();
    let idx = state.evt_index;
    let cycle = state.evt_cycle;
    drop(state);

    let evt = event_ring_virt();
    let e = evt.add(idx).read_volatile();
    let erdp_lo = rt_read(RT_IR0_ERDP_LO);
    let erdp_hi = rt_read(RT_IR0_ERDP_HI);
    crate::console_println!(
        "[xhci] {} event timeout idx={} ccs={} trb_type={} trb_cycle={} status={:#x} control={:#x} usbsts={:#x} iman={:#x} erdp={:#x}{:08x}",
        kind,
        idx,
        if cycle { 1 } else { 0 },
        e.trb_type(),
        if e.cycle() { 1 } else { 0 },
        e.status,
        e.control,
        op_read(OP_USBSTS),
        rt_read(RT_IR0_IMAN),
        erdp_hi,
        erdp_lo
    );
}

// ─── Enumeration ───────────────────────────────────────────────────────

/// Enable a slot for the device on `port`. Returns the slot id.
pub fn enable_slot() -> Option<u8> {
    // Enable Slot command: TRB type = 9, Slot Type = 0 (in bits 16:20).
    let trb = Trb {
        parameter: 0,
        status: 0,
        control: Trb::control(TRB_ENABLE_SLOT, 0),
    };
    let (cc, slot) = submit_command(trb)?;
    if cc != TRB_CC_SUCCESS || slot == 0 {
        crate::console_println!("[xhci] enable_slot cc={} slot={}", cc, slot);
        return None;
    }
    Some(slot)
}

/// Issue an Address Device command for `slot` using the prepared input context.
pub fn address_device(slot: u8) -> bool {
    let ictx_phys = (dma_phys() + INPUT_CTX_OFF) as u64;
    // Address Device: TRB type = 11, Slot ID in bits 24:31, BSR=0 (bit 9).
    let trb = Trb {
        parameter: ictx_phys,
        status: 0,
        control: Trb::control(TRB_ADDRESS_DEV, (slot as u32) << 24),
    };
    match submit_command(trb) {
        Some((cc, _)) => {
            if cc != TRB_CC_SUCCESS {
                crate::console_println!("[xhci] address_device cc={}", cc);
            }
            cc == TRB_CC_SUCCESS
        }
        None => false,
    }
}

/// Issue an Evaluate Context command to update EP0's max packet size after the
/// first 8-byte device descriptor read reveals the real `bMaxPacketSize0`.
///
/// Full-speed devices may declare an EP0 max packet of 8, 16, 32, or 64 bytes
/// (USB 2.0 spec 9.6.1). We address the device with 8 bytes (the only value
/// guaranteed before the descriptor is read); once the descriptor is in, if
/// `bMaxPacketSize0` differs from what we programmed, the controller must be
/// told via Evaluate Context before any longer control transfer. Without this,
/// a Full-speed HID keyboard with EP0=64 stalls the 18-byte descriptor read,
/// which is exactly the "dev desc(18) control transfer failed" seen on real
/// hardware while High-speed devices (EP0=64, addressed with 64) work.
fn evaluate_context_ep0(slot: u8, max_packet0: u16) -> bool {
    unsafe {
        let ictx = input_ctx_virt();
        // Clear the input context arena.
        core::ptr::write_bytes(ictx as *mut u8, 0, INPUT_CONTEXT_SIZE);

        // Evaluate Context only updates the contexts whose add flag is set.
        // We add EP0 only (bit 1); the slot context is unchanged.
        ictx.add(ICTX_A0).write_volatile(0); // drop: none
        ictx.add(ICTX_A1).write_volatile(1 << 1); // add: EP0 (DCI=1)

        // Copy the current output EP0 context, then patch max packet size and
        // clear EP_STATE (input contexts must present EP_STATE=0).
        let out_ep0 = output_ctx_virt(slot).add(output_ep_ctx_dword(1));
        let ep0 = ictx.add(ep_ctx_dword(1));
        for i in 0..context_dwords() {
            ep0.add(i).write_volatile(out_ep0.add(i).read_volatile());
        }
        // Clear EP_STATE (bits 0:2) in ep_info.
        let ep_info = ep0.add(EP_INFO).read_volatile();
        ep0.add(EP_INFO).write_volatile(ep_info & !0x7);
        // Update max packet size (bits 16:31) in ep_info2, preserving
        // CErr/EP-type/max-burst.
        let ep_info2 = ep0.add(EP_INFO2).read_volatile();
        ep0.add(EP_INFO2)
            .write_volatile((ep_info2 & 0xFFFF) | ((max_packet0 as u32) << 16));

        let ictx_phys = (dma_phys() + INPUT_CTX_OFF) as u64;
        // Evaluate Context: TRB type = 13, Slot ID in bits 24:31.
        let trb = Trb {
            parameter: ictx_phys,
            status: 0,
            control: Trb::control(TRB_EVALUATE_CTX, (slot as u32) << 24),
        };
        match submit_command(trb) {
            Some((cc, _)) => {
                if cc != TRB_CC_SUCCESS {
                    crate::console_println!("[xhci] evaluate_context cc={}", cc);
                }
                cc == TRB_CC_SUCCESS
            }
            None => false,
        }
    }
}

/// Enumerate root-hub ports. For each connected device, run a minimal
/// enumeration and, if it is an HID boot keyboard, configure it.
pub fn enumerate_devices() -> bool {
    enumerate_devices_inner(true)
}

fn enumerate_devices_scan() -> bool {
    enumerate_devices_inner(false)
}

fn enumerate_devices_inner(prime_keyboard: bool) -> bool {
    if !is_available() {
        return false;
    }
    SYNC_ENUM_ACTIVE.store(true, Ordering::SeqCst);
    unsafe {
        let iman = rt_read(RT_IR0_IMAN);
        rt_write(RT_IR0_IMAN, iman & !IMAN_IE);
    }
    let ports = XHCI_MAX_PORTS.load(Ordering::Relaxed) as usize;
    if ports == 0 {
        crate::console_println!("[xhci] no ports");
        SYNC_ENUM_ACTIVE.store(false, Ordering::SeqCst);
        unsafe {
            let iman = rt_read(RT_IR0_IMAN);
            rt_write(RT_IR0_IMAN, iman | IMAN_IE);
        }
        return false;
    }
    // Wait for port power to stabilize.
    wait_ms(100);

    let mut connected_ports = 0usize;
    for port in 1..=ports {
        if !port_is_connected(port) {
            continue;
        }
        connected_ports += 1;
        crate::console_println!("[xhci] port {} connected", port);
        let speed = match port_reset(port) {
            Some(s) => s,
            None => {
                crate::console_println!("[xhci] port {} reset failed", port);
                continue;
            }
        };
        port_clear_reset_change(port);
        crate::console_println!("[xhci] port {} speed={:?}", port, speed);

        let slot = match enable_slot() {
            Some(s) => s,
            None => {
                crate::console_println!("[xhci] enable_slot failed");
                continue;
            }
        };
        unsafe {
            set_slot_dcbaa(slot);
        }

        // Determine EP0 max packet size by speed.
        let max_packet0: u16 = match speed {
            UsbSpeed::Low | UsbSpeed::Full => 8,
            UsbSpeed::High => 64,
            UsbSpeed::Super => 512,
            UsbSpeed::Unknown => 8,
        };

        // Build input context and address the device.
        unsafe {
            build_input_context_address(port as u8, speed, max_packet0, input_ctx_virt());
        }
        if !address_device(slot) {
            crate::console_println!("[xhci] address_device failed");
            continue;
        }

        // Read device descriptor (first 8 bytes to get max packet size).
        let mut dev_desc_buf = [0u8; 18];
        let setup = SetupPacket::new(
            USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
            USB_REQ_GET_DESCRIPTOR,
            (USB_DESC_DEVICE as u16) << 8,
            0,
            8,
        );
        match control_transfer(slot, setup, Some((&mut dev_desc_buf[..8], true))) {
            Ok(_) => {}
            Err(e) => {
                crate::console_println!("[xhci] dev desc8: {}", e);
                continue;
            }
        }
        // The 8-byte descriptor's byte 7 is bMaxPacketSize0. Full-speed devices
        // may declare 8/16/32/64; we addressed the device with 8. If the real
        // value differs, issue Evaluate Context to update EP0 before any longer
        // transfer, otherwise the 18-byte read stalls on real hardware.
        let reported_mps0 = dev_desc_buf[7] as u16;
        let valid_mps0 = matches!(reported_mps0, 8 | 16 | 32 | 64 | 9);
        if valid_mps0 && reported_mps0 != max_packet0 {
            crate::console_println!(
                "[xhci] slot {} EP0 mps {}->{} evaluate context",
                slot,
                max_packet0,
                reported_mps0
            );
            if !evaluate_context_ep0(slot, reported_mps0) {
                crate::console_println!("[xhci] evaluate_context failed; proceeding with old mps");
            }
        }
        // Read full 18-byte device descriptor.
        let setup = SetupPacket::new(
            USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
            USB_REQ_GET_DESCRIPTOR,
            (USB_DESC_DEVICE as u16) << 8,
            0,
            18,
        );
        match control_transfer(slot, setup, Some((&mut dev_desc_buf, true))) {
            Ok(_) => {}
            Err(e) => {
                crate::console_println!("[xhci] dev desc18: {}", e);
                continue;
            }
        }
        let vid = dev_desc_buf[8] as u16 | ((dev_desc_buf[9] as u16) << 8);
        let pid = dev_desc_buf[10] as u16 | ((dev_desc_buf[11] as u16) << 8);
        crate::console_println!(
            "[xhci] USB {:04x}:{:04x} class={:#x} subclass={:#x} proto={:#x}",
            vid,
            pid,
            dev_desc_buf[4],
            dev_desc_buf[5],
            dev_desc_buf[6]
        );

        // Read configuration descriptor (first 9 bytes for total length).
        let mut cfg_buf = alloc::vec![0u8; 4096];
        let setup = SetupPacket::new(
            USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
            USB_REQ_GET_DESCRIPTOR,
            (USB_DESC_CONFIGURATION as u16) << 8,
            0,
            9,
        );
        match control_transfer(slot, setup, Some((&mut cfg_buf[..9], true))) {
            Ok(_) => {}
            Err(e) => {
                crate::console_println!("[xhci] cfg desc9: {}", e);
                continue;
            }
        }
        let total = (cfg_buf[2] as usize) | ((cfg_buf[3] as usize) << 8);
        if total < 9 || total > cfg_buf.len() {
            crate::console_println!("[xhci] cfg total {} invalid", total);
            continue;
        }
        let setup = SetupPacket::new(
            USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
            USB_REQ_GET_DESCRIPTOR,
            (USB_DESC_CONFIGURATION as u16) << 8,
            0,
            total as u16,
        );
        match control_transfer(slot, setup, Some((&mut cfg_buf[..total], true))) {
            Ok(_) => {}
            Err(e) => {
                crate::console_println!("[xhci] cfg desc full: {}", e);
                continue;
            }
        }

        let parsed = match ParsedConfiguration::parse(&cfg_buf[..total]) {
            Some(p) => p,
            None => {
                crate::console_println!("[xhci] cfg parse failed");
                continue;
            }
        };
        if dev_desc_buf[4] == USB_CLASS_HUB
            || parsed
                .interfaces
                .iter()
                .any(|iface| iface.iface.b_interface_class == USB_CLASS_HUB)
        {
            crate::console_println!(
                "[xhci] hub device found on root port {}; downstream hub ports not enumerated yet",
                port
            );
        }

        // Scan mode is used only to choose the controller that owns a keyboard.
        // Do not SET_CONFIGURATION or Configure Endpoint here: doing a full HID
        // bind during the scan and then resetting the selected controller again
        // makes real composite keyboards briefly power/configure, then drop
        // back out of their initialized state. The final selected-controller
        // pass below performs the actual bind exactly once.
        if !prime_keyboard && parsed.find_hid_keyboard().is_some() {
            crate::console_println!("[xhci] HID keyboard candidate on port {} (scan-only)", port);
            HAS_KEYBOARD.store(true, Ordering::Relaxed);
            continue;
        }

        // xHCI and USB device configuration must be synchronized. For standard
        // boot keyboards, configure the xHC interrupt endpoint first, then send
        // USB SET_CONFIGURATION, then switch the interface to boot protocol.
        // Linux follows this ordering through usb_hcd_alloc_bandwidth() before
        // usb_control_msg(SET_CONFIGURATION). Configuring the endpoint after
        // the device is already configured can leave some real controllers with
        // a Running periodic endpoint that never gets scheduled.
        if let Some((iface, ep)) = parsed.find_hid_keyboard() {
            crate::console_println!(
                "[xhci] HID keyboard iface={} proto={:#x} ep={:#x}",
                iface.iface.b_interface_number,
                iface.iface.b_interface_protocol,
                ep.b_endpoint_address
            );
            if !configure_configuration_interrupt_in_endpoints(slot, speed, &parsed, ep) {
                crate::console_println!("[xhci] configure_endpoint failed");
                continue;
            }

            let setup = SetupPacket::new(
                USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
                USB_REQ_SET_CONFIGURATION,
                parsed.config.b_configuration_value as u16,
                0,
                0,
            );
            if control_transfer(slot, setup, None).is_err() {
                crate::console_println!("[xhci] set_configuration failed");
                continue;
            }

            initialize_boot_hid_interfaces(slot, &parsed);

            KEYBOARD_SLOT.store(slot, Ordering::Relaxed);
            KEYBOARD_EP_DCI.store(endpoint_dci(ep.number(), true), Ordering::Relaxed);
            KEYBOARD_IFACE.store(iface.iface.b_interface_number, Ordering::Relaxed);
            KEYBOARD_PROTO.store(iface.iface.b_interface_protocol, Ordering::Relaxed);
            KEYBOARD_REPORT_LEN.store(ep.max_packet_size().max(8), Ordering::Relaxed);
            KBD_GET_REPORT_DISABLED.store(false, Ordering::Release);
            HAS_KEYBOARD.store(true, Ordering::Relaxed);
            crate::console_println!("[xhci] keyboard ready slot {}", slot);
            continue;
        }

        // Non-boot HID fallback and non-keyboard devices: keep the simpler
        // legacy order. The report-descriptor fallback may need a configured
        // interface before class-specific descriptor reads.
        let setup = SetupPacket::new(
            USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
            USB_REQ_SET_CONFIGURATION,
            parsed.config.b_configuration_value as u16,
            0,
            0,
        );
        if control_transfer(slot, setup, None).is_err() {
            crate::console_println!("[xhci] set_configuration failed");
            continue;
        }

        if let Some((iface, ep)) = find_report_descriptor_keyboard(slot, &parsed) {
            crate::console_println!(
                "[xhci] HID keyboard iface={} proto={:#x} ep={:#x}",
                iface.iface.b_interface_number,
                iface.iface.b_interface_protocol,
                ep.b_endpoint_address
            );
            if !configure_configuration_interrupt_in_endpoints(slot, speed, &parsed, ep) {
                crate::console_println!("[xhci] configure_endpoint failed");
                continue;
            }
            KEYBOARD_SLOT.store(slot, Ordering::Relaxed);
            KEYBOARD_EP_DCI.store(endpoint_dci(ep.number(), true), Ordering::Relaxed);
            KEYBOARD_IFACE.store(iface.iface.b_interface_number, Ordering::Relaxed);
            KEYBOARD_PROTO.store(iface.iface.b_interface_protocol, Ordering::Relaxed);
            KEYBOARD_REPORT_LEN.store(ep.max_packet_size().max(8), Ordering::Relaxed);
            KBD_GET_REPORT_DISABLED.store(false, Ordering::Release);
            HAS_KEYBOARD.store(true, Ordering::Relaxed);
            crate::console_println!("[xhci] keyboard ready slot {}", slot);
        }
    }
    let found_keyboard = HAS_KEYBOARD.load(Ordering::Relaxed);
    if !found_keyboard {
        if connected_ports == 0 {
            crate::console_println!("[xhci] no connected root ports");
        }
        crate::console_println!("[xhci] no keyboard found");
    }
    SYNC_ENUM_ACTIVE.store(false, Ordering::SeqCst);
    unsafe {
        let iman = rt_read(RT_IR0_IMAN);
        rt_write(RT_IR0_IMAN, iman | IMAN_IE);
    }
    if found_keyboard && prime_keyboard {
        // Kick off the first interrupt-IN transfer only after synchronous
        // enumeration is complete. This prevents a candidate keyboard endpoint
        // found early in the scan from injecting asynchronous transfer events
        // while later devices/controllers are still being enumerated.
        prime_keyboard_transfer();
    }
    found_keyboard
}

fn find_report_descriptor_keyboard<'a>(
    slot: u8,
    parsed: &'a ParsedConfiguration,
) -> Option<(&'a ParsedInterface, &'a EndpointDescriptor)> {
    for iface in &parsed.interfaces {
        if iface.iface.b_interface_class != USB_CLASS_HID {
            continue;
        }
        let Some(ep) = iface
            .endpoints
            .iter()
            .find(|ep| ep.direction_in() && ep.transfer_type() == USB_ENDPOINT_XFER_INT)
        else {
            continue;
        };
        let Some(hid) = iface.hid else {
            continue;
        };
        let report_len = (hid.w_descriptor_length as usize).clamp(1, 512);
        let mut report = alloc::vec![0u8; report_len];
        let setup = SetupPacket::new(
            USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE,
            USB_REQ_GET_DESCRIPTOR,
            (USB_DESC_HID_REPORT as u16) << 8,
            iface.iface.b_interface_number as u16,
            report_len as u16,
        );
        match control_transfer(slot, setup, Some((&mut report, true))) {
            Ok(n) => {
                let len = (n as usize).min(report.len());
                let has_keyboard = hid_report_has_keyboard_usage(&report[..len]);
                if has_keyboard {
                    return Some((iface, ep));
                }
            }
            Err(e) => {
                crate::console_println!(
                    "[xhci] hid report iface={} read failed: {}",
                    iface.iface.b_interface_number,
                    e
                );
            }
        }
    }
    None
}

fn initialize_boot_hid_interfaces(slot: u8, parsed: &ParsedConfiguration) {
    for iface in &parsed.interfaces {
        if iface.iface.b_interface_class != USB_CLASS_HID
            || iface.iface.b_interface_subclass != 0x01
            || !matches!(iface.iface.b_interface_protocol, 0x01 | 0x02)
        {
            continue;
        }

        let iface_num = iface.iface.b_interface_number as u16;
        let setup = SetupPacket::new(
            USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
            HID_REQ_SET_IDLE,
            0,
            iface_num,
            0,
        );
        if let Err(e) = control_transfer(slot, setup, None) {
            crate::console_println!(
                "[xhci] hid init iface={} set_idle failed: {}",
                iface.iface.b_interface_number,
                e
            );
        }

        if iface.iface.b_interface_protocol == 0x01 {
            // Match Linux usbhid_set_leds(): some boot keyboards do not start
            // behaving normally until the host has initialized the LED output
            // report. For a boot keyboard without a Report ID this is a single
            // byte where Num/Caps/Scroll/etc. are all cleared.
            let mut led_report = [0u8; 1];
            let setup = SetupPacket::new(
                USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
                HID_REQ_SET_REPORT,
                2 << 8, // Output report, report ID 0
                iface_num,
                led_report.len() as u16,
            );
            if let Err(e) = control_transfer(slot, setup, Some((&mut led_report, false))) {
                crate::console_println!(
                    "[xhci] hid init iface={} set_leds failed: {}",
                    iface.iface.b_interface_number,
                    e
                );
            }
        }
    }
}

/// Compute the xHCI Device Context Index for an endpoint.
/// DCI = 2*ep_number + (1 if IN, 0 if OUT). EP0 is DCI=1.
pub fn endpoint_dci(ep_number: u8, dir_in: bool) -> u8 {
    if ep_number == 0 {
        1
    } else {
        2 * ep_number + if dir_in { 1 } else { 0 }
    }
}

/// Issue a Configure Endpoint command for the active configuration's interrupt
/// IN endpoints. The USB SET_CONFIGURATION request activates all endpoints in
/// the selected configuration; real xHCs expect their periodic scheduling state
/// to reflect that configuration, not just the one endpoint we plan to read.
fn configure_configuration_interrupt_in_endpoints(
    slot: u8,
    speed: UsbSpeed,
    parsed: &ParsedConfiguration,
    keyboard_ep: &EndpointDescriptor,
) -> bool {
    unsafe {
        let mut endpoints = alloc::vec::Vec::<EndpointDescriptor>::new();
        for iface in &parsed.interfaces {
            for ep in &iface.endpoints {
                if ep.direction_in() && ep.transfer_type() == USB_ENDPOINT_XFER_INT {
                    endpoints.push(*ep);
                }
            }
        }
        if endpoints.is_empty() {
            return false;
        }

        let ictx = input_ctx_virt();
        // Clear input context.
        core::ptr::write_bytes(ictx as *mut u8, 0, INPUT_CONTEXT_SIZE);

        let keyboard_dci = endpoint_dci(keyboard_ep.number(), true);
        let mut add_flags: u32 = 1 << 0;
        let mut highest_dci = keyboard_dci;
        for ep in &endpoints {
            let dci = endpoint_dci(ep.number(), true);
            add_flags |= 1 << dci;
            highest_dci = highest_dci.max(dci);
        }
        ictx.add(ICTX_A0).write_volatile(0); // drop: none
        ictx.add(ICTX_A1).write_volatile(add_flags); // add: slot + interrupt EPs

        // Slot context: copy the current output Slot Context and only update
        // Context Entries. A zeroed slot context loses route/speed/root-port
        // fields; QEMU tolerates that, but real controllers can accept the
        // command and still never schedule the endpoint.
        let slot_base = ictx.add(slot_ctx_dword());
        let out_slot = output_ctx_virt(slot);
        for i in 0..context_dwords() {
            slot_base
                .add(i)
                .write_volatile(out_slot.add(i).read_volatile());
        }
        let cur_dev_info = slot_base.add(SLOT_DEV_INFO).read_volatile();
        slot_base
            .add(SLOT_DEV_INFO)
            .write_volatile((cur_dev_info & !(0x1f << 27)) | (highest_dci as u32) << 27);

        for ep in &endpoints {
            let dci = endpoint_dci(ep.number(), true);
            let ep_ctx = ictx.add(ep_ctx_dword(dci));
            let interval = xhci_interrupt_interval(speed, ep.b_interval);
            let max_packet = ep.max_packet_size() as u32;
            // dword 0 (ep_info): interval in bits 16-23.
            ep_ctx.add(EP_INFO).write_volatile(interval << 16);
            // dword 1 (ep_info2): CErr(1-2)=3, EP type(3-5)=int in (7),
            // max packet(16-31).
            let ep_info2: u32 = (3u32 << 1) | (EP_TYPE_INT_IN << 3) | (max_packet << 16);
            ep_ctx.add(EP_INFO2).write_volatile(ep_info2);
            // dword 2..3 (deq): endpoint-specific transfer ring base, DCS=1.
            let ring_off = if dci == keyboard_dci {
                INT_RING_OFF
            } else {
                endpoint_ring_off(dci)
            };
            let ring_phys = (dma_phys() + ring_off) as u64;
            ep_ctx.add(EP_DEQ_LO).write_volatile(ring_phys as u32 | 1);
            ep_ctx
                .add(EP_DEQ_HI)
                .write_volatile((ring_phys >> 32) as u32);
            // dword 4 (tx_info): average TRB length and Max ESIT Payload.
            let avg_trb_len = max_packet;
            ep_ctx
                .add(EP_TX_INFO)
                .write_volatile(avg_trb_len | (max_packet << 16));
        }

        let ictx_phys = (dma_phys() + INPUT_CTX_OFF) as u64;
        let trb = Trb {
            parameter: ictx_phys,
            status: 0,
            control: Trb::control(TRB_CONFIGURE_EP, (slot as u32) << 24),
        };
        match submit_command(trb) {
            Some((cc, _)) => {
                if cc != TRB_CC_SUCCESS {
                    crate::console_println!("[xhci] configure_ep cc={}", cc);
                    return false;
                }
                let out_ep = output_ctx_virt(slot).add(output_ep_ctx_dword(keyboard_dci));
                let out_info = out_ep.add(EP_INFO).read_volatile();
                let ep_state = out_info & 0x7;
                ep_state == EP_STATE_RUNNING
            }
            None => false,
        }
    }
}

pub fn xhci_interrupt_interval(speed: UsbSpeed, b_interval: u8) -> u32 {
    match speed {
        UsbSpeed::Low | UsbSpeed::Full => {
            // Full/low-speed interrupt bInterval is in 1ms frames. xHCI stores
            // the ESIT exponent in 125us microframes, clamped to at least 1ms.
            let microframes = (b_interval.max(1) as u32).saturating_mul(8);
            let mut interval = 0u32;
            let mut value = microframes;
            while value > 1 {
                value >>= 1;
                interval += 1;
            }
            interval.max(3)
        }
        UsbSpeed::High | UsbSpeed::Super => b_interval.clamp(1, 16) as u32 - 1,
        UsbSpeed::Unknown => 3,
    }
}

// ─── Interrupt endpoint polling ─────────────────────────────────────────

/// Enqueue a single interrupt-IN transfer on the keyboard endpoint
/// and ring the doorbell. Non-blocking: returns immediately after submitting.
/// The matching Transfer Event is consumed in `handle_irq`, which reads the
/// HID report, feeds the TTY, and re-primes the next transfer.
///
/// This is safe to call from the XHCI interrupt handler (and once from
/// `enumerate_devices` to start the stream). It must NOT be called from the
/// timer ISR, because it rings the doorbell and the completion is delivered
/// as a separate XHCI interrupt.
pub fn prime_keyboard_transfer() {
    if !HAS_KEYBOARD.load(Ordering::Relaxed) {
        return;
    }
    let slot = KEYBOARD_SLOT.load(Ordering::Relaxed);
    let dci = KEYBOARD_EP_DCI.load(Ordering::Relaxed);
    if slot == 0 || dci == 0 {
        return;
    }
    // Avoid stacking a second transfer while one is already in flight.
    if KBD_XFER_PENDING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    unsafe {
        let buf_phys = (dma_phys() + INT_DATA_OFF) as u64;
        let ring = int_ring_virt();

        // Enqueue a Normal TRB sized to the endpoint's MaxPacketSize. Composite
        // HID devices often include Report IDs or vendor fields and return
        // reports larger than the 8-byte boot-keyboard format; using a shorter
        // TRB can leave real controllers waiting indefinitely for a TD that
        // does not match the endpoint's periodic payload contract.
        let report_len = (KEYBOARD_REPORT_LEN.load(Ordering::Relaxed) as u32)
            .clamp(8, 256)
            .min(4096);
        let mut state = RING_LOCK.lock();
        let cycle = state.int_cycle;
        let publish_cycle_bit = if cycle { TRB_CYCLE_BIT } else { 0 };
        let invalid_cycle_bit = if cycle { 0 } else { TRB_CYCLE_BIT };
        let publish_control = Trb::control(TRB_NORMAL, TRB_IOC | TRB_ISP) | publish_cycle_bit;
        let trb = Trb {
            parameter: buf_phys,
            status: report_len,
            // Do not give ownership to the xHC until parameter/status are
            // globally visible. Periodic endpoints are already Running and can
            // fetch the ring without a doorbell race window; publish the Cycle
            // bit as the final store, matching Linux's giveback_first_trb().
            control: Trb::control(TRB_NORMAL, TRB_IOC | TRB_ISP) | invalid_cycle_bit,
        };
        if state.int_index == RING_SIZE - 1 {
            let link = ring.add(RING_SIZE - 1).read_volatile();
            let mut new_link = link;
            new_link.control =
                (new_link.control & !TRB_CYCLE_BIT) | if cycle { TRB_CYCLE_BIT } else { 0 };
            ring.add(RING_SIZE - 1).write_volatile(new_link);
            state.int_index = 0;
            state.int_cycle = !state.int_cycle;
        }
        let trb_idx = state.int_index;
        ring.add(trb_idx).write_volatile(trb);
        core::sync::atomic::fence(Ordering::SeqCst);
        core::ptr::addr_of_mut!((*ring.add(trb_idx)).control).write_volatile(publish_control);
        state.int_index += 1;
        drop(state);

        KBD_LAST_NUDGE_TSC.store(read_tsc(), Ordering::Release);
        doorbell_ring(slot as u32, dci as u32);
    }
}

/// Non-blocking fallback used from stdin read paths. This drains any pending
/// XHCI events and re-primes the keyboard interrupt endpoint without relying
/// on PCI interrupt delivery, which is fragile on real firmware/INTx setups.
pub fn poll_keyboard() {
    if !is_available()
        || SYNC_ENUM_ACTIVE.load(Ordering::Relaxed)
        || !HAS_KEYBOARD.load(Ordering::Relaxed)
    {
        return;
    }

    unsafe {
        drain_events();
    }

    if !KBD_XFER_PENDING.load(Ordering::Acquire) {
        prime_keyboard_transfer();
    } else {
        nudge_keyboard_transfer();
    }
}

fn nudge_keyboard_transfer() {
    let now = read_tsc();
    let last = KBD_LAST_NUDGE_TSC.load(Ordering::Acquire);
    if now.wrapping_sub(last) < 10u64.saturating_mul(TSC_CYCLES_PER_MS) {
        return;
    }
    KBD_LAST_NUDGE_TSC.store(now, Ordering::Release);

    let slot = KEYBOARD_SLOT.load(Ordering::Relaxed);
    let dci = KEYBOARD_EP_DCI.load(Ordering::Relaxed);
    if slot == 0 || dci == 0 {
        return;
    }

    unsafe {
        doorbell_ring(slot as u32, dci as u32);
    }
}

fn poll_keyboard_get_report() {
    let now = read_tsc();
    if KBD_GET_REPORT_DISABLED.load(Ordering::Acquire) {
        return;
    }
    let last = KBD_LAST_GET_REPORT_TSC.load(Ordering::Acquire);
    if now.wrapping_sub(last) < 50u64.saturating_mul(TSC_CYCLES_PER_MS) {
        return;
    }
    KBD_LAST_GET_REPORT_TSC.store(now, Ordering::Release);

    let slot = KEYBOARD_SLOT.load(Ordering::Relaxed);
    let iface = KEYBOARD_IFACE.load(Ordering::Relaxed);
    if slot == 0 {
        return;
    }

    let mut report = [0u8; 256];
    let report_len = (KEYBOARD_REPORT_LEN.load(Ordering::Relaxed) as usize).clamp(8, report.len());
    let setup = SetupPacket::new(
        USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
        HID_REQ_GET_REPORT,
        1 << 8, // Input report, report ID 0
        iface as u16,
        report_len as u16,
    );
    match control_transfer(slot, setup, Some((&mut report[..report_len], true))) {
        Ok(_) => unsafe {
            feed_report_bytes(report.as_ptr());
        },
        Err(_) => {
            KBD_GET_REPORT_DISABLED.store(true, Ordering::Release);
        }
    }
}

// ─── IRQ handler ───────────────────────────────────────────────────────

/// Called from the XHCI interrupt ISR. Advances the event ring and processes
/// any pending events. Does NOT perform enumeration or blocking work.
pub fn handle_irq() {
    if !is_available() {
        return;
    }
    unsafe {
        if SYNC_ENUM_ACTIVE.load(Ordering::SeqCst) {
            let iman = rt_read(RT_IR0_IMAN);
            rt_write(RT_IR0_IMAN, iman & !IMAN_IE);
            return;
        }

        // Clear the interrupt pending bit by writing IMAN with IP=0 (and IE=1).
        let iman = rt_read(RT_IR0_IMAN);
        rt_write(RT_IR0_IMAN, (iman & !IMAN_IP) | IMAN_IE);

        drain_events();
    }
}

unsafe fn drain_events() {
    let evt = event_ring_virt();
    loop {
        let state = RING_LOCK.lock();
        let idx = state.evt_index;
        let consumer_cycle = state.evt_cycle;
        drop(state);

        let e = evt.add(idx).read_volatile();
        if e.cycle() != consumer_cycle {
            break;
        }

        process_event(e);
        advance_event_ring();
    }
}

unsafe fn process_event(e: Trb) {
    match e.trb_type() {
        TRB_TRANSFER_EVENT => {
            let slot = e.slot_id();
            let ep = e.endpoint_id();
            let cc = e.completion_code();
            if HAS_KEYBOARD.load(Ordering::Relaxed)
                && slot == KEYBOARD_SLOT.load(Ordering::Relaxed)
                && ep == KEYBOARD_EP_DCI.load(Ordering::Relaxed)
            {
                if cc == TRB_CC_SUCCESS || cc == TRB_CC_SHORT_PACKET {
                    read_keyboard_report_and_feed();
                }
                KBD_XFER_PENDING.store(false, Ordering::SeqCst);
                prime_keyboard_transfer();
            }
        }
        TRB_CMD_COMPLETE => {
            // Command complete events are consumed by synchronous enumeration.
        }
        TRB_PORT_STATUS_CHANGE => {
            handle_root_hub_port_changes();
        }
        _ => {}
    }
}

/// Read the 8-byte HID boot keyboard report from the interrupt data buffer
/// and feed any newly-pressed keys into the TTY. Called only from
/// `handle_irq` (XHCI interrupt context) after a successful transfer.
unsafe fn read_keyboard_report_and_feed() {
    let rpt = int_data_virt();
    feed_report_bytes(rpt);
}

unsafe fn feed_report_bytes(rpt: *const u8) {
    let modifier = rpt.read_volatile();
    for i in 0..6u8 {
        let code = rpt.add(2 + i as usize).read_volatile();
        if code != 0 {
            let ascii = super::hid::hid_to_ascii(code, modifier);
            if ascii != 0 {
                super::hid::feed_to_tty(ascii);
            }
        }
    }
}

// ─── Timing helper (pre-LAPIC-timer) ───────────────────────────────────

fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
    }
    ((hi as u64) << 32) | (lo as u64)
}

fn wait_ms(ms: u64) {
    let target = ms.saturating_mul(TSC_CYCLES_PER_MS);
    let start = read_tsc();
    loop {
        let now = read_tsc();
        if now.wrapping_sub(start) >= target {
            return;
        }
        core::hint::spin_loop();
    }
}

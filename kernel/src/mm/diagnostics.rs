//! Structured memory diagnostics for page fault analysis.
//!
//! Typed diagnostic events that identify the failing boundary, not just the symptom.
//! All logs print unconditionally; filtering happens offline.

use crate::mm::addr::PhysAddr;
use crate::mm::page_table::WalkResult;

/// What kind of access triggered the page fault.
#[derive(Debug, Clone, Copy)]
pub enum FaultAccess {
    Read,
    Write,
    Execute,
}

/// Typed page fault event for diagnostic logging.
#[derive(Debug)]
pub struct PageFaultEvent {
    pub pid: usize,
    pub slot: usize,
    pub rip: usize,
    pub fault_addr: usize,
    pub access: FaultAccess,
    pub active_cr3: usize,
    pub expected_root: usize,
    pub fs_base: u64,
    pub walk_result: Option<WalkResult>,
    pub vma_prot: Option<usize>,
}

impl PageFaultEvent {
    /// Log the full event to the kernel console.
    pub fn log(&self) {
        let access_str = match self.access {
            FaultAccess::Read => "R",
            FaultAccess::Write => "W",
            FaultAccess::Execute => "X",
        };
        let walk_str = match &self.walk_result {
            Some(WalkResult::NotMapped) => "NotMapped".into(),
            Some(WalkResult::Mapped4K { frame, flags }) => {
                alloc::format!("4K frame={:?} flags={:#x}", frame, flags)
            }
            Some(WalkResult::MappedHuge { frame, level, size, flags }) => {
                alloc::format!("Huge frame={:?} L={} sz={} flags={:#x}", frame, level, size, flags)
            }
            Some(WalkResult::Invalid) => "Invalid".into(),
            None => "NoWalk".into(),
        };
        crate::console_println!(
            "[PF] pid={} slot={} rip={:#x} fault={:#x} {} cr3={:#x} root={:#x} fs={:#x} walk={} vma={}",
            self.pid,
            self.slot,
            self.rip,
            self.fault_addr,
            access_str,
            self.active_cr3,
            self.expected_root,
            self.fs_base,
            walk_str,
            match self.vma_prot {
                Some(p) => alloc::format!("{}", p),
                None => "None".into(),
            }
        );

        // Extra context for low-address faults (likely TLS loss)
        if self.fault_addr < 0x1000 {
            crate::console_println!(
                "[PF] LOW-ADDR: fs_base={:#x} rip={:#x} — likely TLS dereference after incomplete user-return",
                self.fs_base,
                self.rip
            );
        }
    }
}

/// PTE chain dump for a virtual address in a given page table.
/// Returns a formatted string with all 4 levels.
pub fn dump_pte_chain(root: usize, vaddr: usize) -> PteChain {
    PteChain::walk(root, vaddr)
}

/// Result of a PTE chain walk for debugging.
#[derive(Debug)]
pub struct PteChain {
    pub pml4e: u64,
    pub pdpe: u64,
    pub pde: u64,
    pub pte: u64,
    pub leaf_kind: &'static str,
    pub translated_frame: Option<usize>,
}

impl PteChain {
    #[cfg(target_arch = "x86_64")]
    fn walk(root: usize, vaddr: usize) -> Self {
        use crate::mm::vmm::{PageTable, PTEFlags};

        fn vpn(addr: usize, level: usize) -> usize {
            (addr >> (12 + 9 * level)) & 0x1FF
        }

        let root_table = unsafe { &mut *(root as *mut PageTable) };
        let pml4_idx = vpn(vaddr, 3);
        let pml4e = root_table.entries[pml4_idx].0;

        if !root_table.entries[pml4_idx].is_valid() {
            return Self {
                pml4e,
                pdpe: 0,
                pde: 0,
                pte: 0,
                leaf_kind: "PML4 NotPresent",
                translated_frame: None,
            };
        }

        let pdp = unsafe { &mut *((root_table.entries[pml4_idx].ppn() << 12) as *mut PageTable) };
        let pdp_idx = vpn(vaddr, 2);
        let pdpe = pdp.entries[pdp_idx].0;

        if !pdp.entries[pdp_idx].is_valid() {
            return Self {
                pml4e,
                pdpe,
                pde: 0,
                pte: 0,
                leaf_kind: "PDP NotPresent",
                translated_frame: None,
            };
        }
        if pdp.entries[pdp_idx].flags().contains(PTEFlags::PS) {
            return Self {
                pml4e,
                pdpe,
                pde: 0,
                pte: 0,
                leaf_kind: "1GB HugePage",
                translated_frame: Some(pdp.entries[pdp_idx].ppn() << 12),
            };
        }

        let pd = unsafe { &mut *((pdp.entries[pdp_idx].ppn() << 12) as *mut PageTable) };
        let pd_idx = vpn(vaddr, 1);
        let pde = pd.entries[pd_idx].0;

        if !pd.entries[pd_idx].is_valid() {
            return Self {
                pml4e,
                pdpe,
                pde,
                pte: 0,
                leaf_kind: "PD NotPresent",
                translated_frame: None,
            };
        }
        if pd.entries[pd_idx].flags().contains(PTEFlags::PS) {
            return Self {
                pml4e,
                pdpe,
                pde,
                pte: 0,
                leaf_kind: "2MB HugePage",
                translated_frame: Some(pd.entries[pd_idx].ppn() << 12),
            };
        }

        let pt = unsafe { &mut *((pd.entries[pd_idx].ppn() << 12) as *mut PageTable) };
        let pt_idx = vpn(vaddr, 0);
        let pte = pt.entries[pt_idx].0;

        if !pt.entries[pt_idx].is_valid() {
            return Self {
                pml4e,
                pdpe,
                pde,
                pte,
                leaf_kind: "PT NotPresent",
                translated_frame: None,
            };
        }

        Self {
            pml4e,
            pdpe,
            pde,
            pte,
            leaf_kind: "4K",
            translated_frame: Some(pt.entries[pt_idx].ppn() << 12),
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn walk(_root: usize, _vaddr: usize) -> Self {
        Self {
            pml4e: 0, pdpe: 0, pde: 0, pte: 0,
            leaf_kind: "N/A",
            translated_frame: None,
        }
    }
}

/// Log a page-table mutation event.
pub fn log_pt_mutation(op: &str, pid: usize, root: usize, va: usize, before: &str, after: &str, result: &str) {
    crate::console_println!(
        "[pt-mut] op={} pid={} root={:#x} va={:#x} before={} after={} result={}",
        op, pid, root, va, before, after, result
    );
}

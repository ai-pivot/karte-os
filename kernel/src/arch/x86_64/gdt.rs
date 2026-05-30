//! GDT (Global Descriptor Table) setup using the `x86_64` crate.
//!
//! On x86_64, the GDT is mostly legacy — segmentation is effectively disabled
//! in long mode. We still need it for:
//! - Kernel code/data segments
//! - User code/data segments (Ring 3)
//! - TSS (Task State Segment) for IST (Interrupt Stack Table) entries
//!   used by double fault handling

use core::sync::atomic::Ordering;

use spin::Once;
use x86_64::VirtAddr;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

/// IST index for double fault handler (separate stack to avoid corruption).
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Number of IST entries we actually use.
const IST_STACK_SIZE: usize = 4096 * 8; // 32KB per IST stack

/// IST stacks — placed in BSS, always accessible.
/// We use `spin::Once` pattern: static mutable data behind an init guard.
static mut IST_STACKS: [[u8; IST_STACK_SIZE]; 1] = [[0u8; IST_STACK_SIZE]; 1];

/// Task State Segment — provides kernel stack for privilege level changes
/// and IST stacks for specific exceptions.
static mut TSS: TaskStateSegment = TaskStateSegment::new();

/// GDT + cached selectors.
struct GdtStruct {
    gdt: GlobalDescriptorTable,
    code: SegmentSelector,
    data: SegmentSelector,
    user_code: SegmentSelector,
    user_data: SegmentSelector,
    tss: SegmentSelector,
}

static GDT: Once<GdtStruct> = Once::new();

/// User code segment selector value (for iretq).
/// After GDT init, this is set to the actual selector.
pub static USER_CODE_SEL: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
/// User data segment selector value (for iretq).
pub static USER_DATA_SEL: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
/// Kernel code segment selector value.
pub static KCODE_SEL: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);

/// Initialize the GDT and load it into the GDTR.
/// Also loads the TSS and all segment registers.
///
/// # Safety
/// Must be called exactly once during early boot, before any interrupt
/// or user-mode code is used.
pub fn init() {
    GDT.call_once(|| {
        // Set up IST[0] for double fault
        unsafe {
            let stack_top =
                VirtAddr::new(IST_STACKS[0].as_ptr() as u64 + IST_STACKS[0].len() as u64);
            TSS.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = stack_top;
        }

        let mut gdt = GlobalDescriptorTable::new();

        // Entry 0x08: kernel code (64-bit, DPL=0)
        let code = gdt.append(Descriptor::kernel_code_segment());
        // Entry 0x10: kernel data (64-bit, DPL=0)
        let data = gdt.append(Descriptor::kernel_data_segment());
        // Entry 0x28: user code (64-bit, DPL=3)
        let user_code = gdt.append(Descriptor::user_code_segment());
        // Entry 0x30: user data (64-bit, DPL=3)
        let user_data = gdt.append(Descriptor::user_data_segment());
        // TSS descriptor (takes two GDT slots on x86_64)
        let tss = unsafe { gdt.append(Descriptor::tss_segment(&*core::ptr::addr_of!(TSS))) };

        // Cache selector values for use in iretq frames and assembly
        USER_CODE_SEL.store(user_code.0, Ordering::Relaxed);
        USER_DATA_SEL.store(user_data.0, Ordering::Relaxed);
        KCODE_SEL.store(code.0, Ordering::Relaxed);

        GdtStruct {
            gdt,
            code,
            data,
            user_code,
            user_data,
            tss,
        }
    });

    let gdt = GDT.get().unwrap();

    // Load GDTR
    gdt.gdt.load();

    // Reload all segment registers + TSS
    unsafe {
        use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS, Segment};
        use x86_64::instructions::tables::load_tss;

        CS::set_reg(gdt.code);
        DS::set_reg(gdt.data);
        ES::set_reg(gdt.data);
        FS::set_reg(gdt.data);
        GS::set_reg(gdt.data);
        SS::set_reg(gdt.data);
        load_tss(gdt.tss);
    }
}

/// Update the TSS.RSP0 (kernel stack pointer for Ring 3 → Ring 0 transitions).
/// Must be called before entering user mode and on every context switch.
///
/// # Safety
/// The caller must ensure `kernel_stack_top` points to valid, accessible memory.
pub unsafe fn set_kernel_rsp0(kernel_stack_top: u64) {
    // TSS is initialized by init() which runs first
    unsafe {
        TSS.privilege_stack_table[0] = x86_64::VirtAddr::new(kernel_stack_top);
    }
}
